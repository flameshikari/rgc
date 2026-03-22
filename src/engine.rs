// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Colorization engine: applies rules to input lines and emits ANSI-colored output.
//!
//! Uses a per-character color index array for correct overlap handling
//! and a pre-built palette to avoid repeated ANSI sequence construction.

use std::io::Write;

use crate::color::{ColorSpec, RESET};
use crate::config::{CompiledRegex, CountMode, Rule};

const MAX_GROUPS: usize = 16;

/// The core colorization engine. Holds compiled rules and reusable buffers.
pub struct Engine<'a> {
    rules: &'a [Rule],
    fired: Vec<bool>,
    block_active: bool,
    block_color_idx: u16,
    prev_count: CountMode,
    palette: Vec<&'a [u8]>,
    palette_is_color: Vec<bool>,
    colors_flat: Vec<u16>,
    rule_color_ranges: Vec<(u16, u16)>,
    has_replace: bool,
    spans: Vec<Span>,
    char_colors: Vec<u16>,
    output_buf: Vec<u8>,
    replace_buf: String,
}

#[derive(Clone, Copy)]
struct Span {
    start: u32,
    end: u32,
    color_idx: u16,
}

struct MatchResult {
    start: usize,
    end: usize,
    group_count: usize,
    groups: [(u32, u32); MAX_GROUPS],
    group_valid: u16,
}

impl<'a> Engine<'a> {
    /// Build an engine from compiled rules. Pre-computes the color palette.
    pub fn new(rules: &'a [Rule]) -> Self {
        let mut palette: Vec<&[u8]> = Vec::with_capacity(rules.len() * 4);
        let mut palette_is_color = Vec::with_capacity(rules.len() * 4);
        palette.push(RESET);
        palette_is_color.push(true);

        let mut colors_flat: Vec<u16> = Vec::with_capacity(rules.len() * 4);
        let mut rule_color_ranges = Vec::with_capacity(rules.len());
        let has_replace = rules.iter().any(|r| r.replace.is_some());

        for rule in rules {
            let start = colors_flat.len() as u16;
            for spec in &rule.colors {
                let idx = palette.len() as u16;
                match spec {
                    ColorSpec::Ansi(bytes) => {
                        palette.push(bytes);
                        palette_is_color.push(true);
                    }
                    _ => {
                        palette.push(&[]);
                        palette_is_color.push(false);
                    }
                }
                colors_flat.push(idx);
            }
            rule_color_ranges.push((start, rule.colors.len() as u16));
        }

        Self {
            fired: vec![false; rules.len()],
            rules,
            block_active: false,
            block_color_idx: 0,
            prev_count: CountMode::More,
            palette,
            palette_is_color,
            colors_flat,
            rule_color_ranges,
            has_replace,
            spans: Vec::with_capacity(64),
            char_colors: Vec::with_capacity(1024),
            output_buf: Vec::with_capacity(4096),
            replace_buf: String::new(),
        }
    }

    /// Colorize a single line and write the result (with `\r\n`) to `out`.
    pub fn colorize_line<W: Write>(&mut self, line: &str, out: &mut W) {
        if line.is_empty() {
            let _ = out.write_all(b"\r\n");
            return;
        }

        self.spans.clear();

        if self.has_replace {
            self.replace_buf.clear();
            self.replace_buf.push_str(line);
            self.apply_rules_replace();
            let tmp = std::mem::take(&mut self.replace_buf);
            self.emit_colorized(&tmp, out);
            self.replace_buf = tmp;
        } else {
            self.apply_rules(line);
            self.emit_colorized(line, out);
        }
    }

    fn apply_rules(&mut self, line: &str) {
        let line_len = line.len();
        for ri in 0..self.rules.len() {
            let rule = &self.rules[ri];
            if rule.count == CountMode::Once && self.fired[ri] { continue; }

            let eff = if rule.count == CountMode::Previous { self.prev_count } else { rule.count };

            if self.block_active && eff != CountMode::Unblock && self.block_color_idx != 0 {
                self.spans.push(Span { start: 0, end: line_len as u32, color_idx: self.block_color_idx });
            }

            let (cs, cl) = self.rule_color_ranges[ri];
            let mut pos = 0;
            let mut found = false;

            loop {
                if pos > line_len { break; }
                let Some(m) = match_at(&self.rules[ri].regex, line, pos) else { break };
                found = true;

                if self.rules[ri].skip { return; }

                self.push_spans(&m, cs, cl);

                match eff {
                    CountMode::More => { pos = if m.end == pos { pos + 1 } else { m.end }; }
                    CountMode::Once => { self.fired[ri] = true; break; }
                    CountMode::Stop => break,
                    CountMode::Block => {
                        self.block_active = true;
                        if cl > 0 { self.block_color_idx = self.colors_flat[cs as usize]; }
                        break;
                    }
                    CountMode::Unblock => { self.block_active = false; self.block_color_idx = 0; break; }
                    CountMode::Previous => unreachable!(),
                }
            }

            if eff != CountMode::Previous { self.prev_count = eff; }
            if found && eff == CountMode::Stop { break; }
        }
    }

    fn apply_rules_replace(&mut self) {
        for ri in 0..self.rules.len() {
            let rule = &self.rules[ri];
            if rule.count == CountMode::Once && self.fired[ri] { continue; }

            let eff = if rule.count == CountMode::Previous { self.prev_count } else { rule.count };

            let line_len = self.replace_buf.len();
            if self.block_active && eff != CountMode::Unblock && self.block_color_idx != 0 {
                self.spans.push(Span { start: 0, end: line_len as u32, color_idx: self.block_color_idx });
            }

            let (cs, cl) = self.rule_color_ranges[ri];
            let mut pos = 0;
            let mut found = false;

            loop {
                if pos > self.replace_buf.len() { break; }
                let Some(m) = match_at(&self.rules[ri].regex, &self.replace_buf, pos) else { break };
                found = true;

                if self.rules[ri].skip { return; }

                if let Some(ref repl) = self.rules[ri].replace {
                    let new = replace_match(&self.rules[ri].regex, &self.replace_buf, repl);
                    self.replace_buf = new;
                    break;
                }

                self.push_spans(&m, cs, cl);

                match eff {
                    CountMode::More => { pos = if m.end == pos { pos + 1 } else { m.end }; }
                    CountMode::Once => { self.fired[ri] = true; break; }
                    CountMode::Stop => break,
                    CountMode::Block => {
                        self.block_active = true;
                        if cl > 0 { self.block_color_idx = self.colors_flat[cs as usize]; }
                        break;
                    }
                    CountMode::Unblock => { self.block_active = false; self.block_color_idx = 0; break; }
                    CountMode::Previous => unreachable!(),
                }
            }

            if eff != CountMode::Previous { self.prev_count = eff; }
            if found && eff == CountMode::Stop { break; }
        }
    }

    #[inline]
    fn push_spans(&mut self, m: &MatchResult, cs: u16, cl: u16) {
        if cl > 0 {
            self.spans.push(Span {
                start: m.start as u32,
                end: m.end as u32,
                color_idx: self.colors_flat[cs as usize],
            });
        }
        let gc = m.group_count.min((cl as usize).saturating_sub(1));
        for gi in 0..gc {
            if m.group_valid & (1 << gi) != 0 {
                self.spans.push(Span {
                    start: m.groups[gi].0,
                    end: m.groups[gi].1,
                    color_idx: self.colors_flat[cs as usize + gi + 1],
                });
            }
        }
    }

    fn emit_colorized<W: Write>(&mut self, line: &str, out: &mut W) {
        if self.spans.is_empty() {
            let _ = out.write_all(line.as_bytes());
            let _ = out.write_all(b"\r\n");
            return;
        }

        let len = line.len();
        self.char_colors.clear();
        self.char_colors.resize(len, 0);

        for &span in &self.spans {
            let start = (span.start as usize).min(len);
            let end = (span.end as usize).min(len);
            if self.palette_is_color[span.color_idx as usize] {
                // Use fill for potential SIMD optimization
                self.char_colors[start..end].fill(span.color_idx);
            }
        }

        self.output_buf.clear();
        let bytes = line.as_bytes();
        let mut cur: u16 = u16::MAX;

        for (i, &byte) in bytes.iter().enumerate() {
            let wanted = self.char_colors[i];
            if wanted != cur {
                if cur != 0 && cur != u16::MAX {
                    self.output_buf.extend_from_slice(RESET);
                }
                if wanted != 0 {
                    self.output_buf.extend_from_slice(self.palette[wanted as usize]);
                }
                cur = wanted;
            }
            self.output_buf.push(byte);
        }

        if cur != 0 && cur != u16::MAX {
            self.output_buf.extend_from_slice(RESET);
        }
        self.output_buf.extend_from_slice(b"\r\n");
        let _ = out.write_all(&self.output_buf);
    }
}

#[inline(always)]
fn match_at(regex: &CompiledRegex, text: &str, pos: usize) -> Option<MatchResult> {
    match regex {
        CompiledRegex::Fast(re) => {
            let caps = re.captures_at(text, pos)?;
            let m = caps.get(0)?;
            let gc = caps.len().saturating_sub(1).min(MAX_GROUPS);
            let mut r = MatchResult {
                start: m.start(), end: m.end(), group_count: gc,
                groups: [(0, 0); MAX_GROUPS], group_valid: 0,
            };
            for i in 0..gc {
                if let Some(g) = caps.get(i + 1) {
                    r.groups[i] = (g.start() as u32, g.end() as u32);
                    r.group_valid |= 1 << i;
                }
            }
            Some(r)
        }
        CompiledRegex::Fancy(re) => {
            let text = if pos > 0 { &text[pos..] } else { text };
            let caps = re.captures(text).ok()??;
            let m = caps.get(0)?;
            let gc = caps.len().saturating_sub(1).min(MAX_GROUPS);
            let mut r = MatchResult {
                start: m.start() + pos, end: m.end() + pos, group_count: gc,
                groups: [(0, 0); MAX_GROUPS], group_valid: 0,
            };
            for i in 0..gc {
                if let Some(g) = caps.get(i + 1) {
                    r.groups[i] = ((g.start() + pos) as u32, (g.end() + pos) as u32);
                    r.group_valid |= 1 << i;
                }
            }
            Some(r)
        }
    }
}

fn replace_match(regex: &CompiledRegex, text: &str, replacement: &str) -> String {
    match regex {
        CompiledRegex::Fast(re) => re.replace(text, replacement).into_owned(),
        CompiledRegex::Fancy(re) => re.replace(text, replacement).into_owned(),
    }
}
