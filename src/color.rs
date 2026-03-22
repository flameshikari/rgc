// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! ANSI color parsing: converts human-readable color specs into escape sequences.
//!
//! Supports standard 16 colors, 256-color, RGB, modifiers (bold, italic, ...),
//! and a palette system for reusable color aliases.

use std::collections::HashMap;

/// Color specification for a capture group.
#[derive(Clone, Debug)]
pub enum ColorSpec {
    Ansi(Vec<u8>),
    Unchanged,
    Previous,
}

/// Parse a color spec, resolving palette references.
pub fn parse_color_spec_with_palette(spec: &str, palette: &HashMap<String, String>) -> ColorSpec {
    let spec = spec.trim();

    // Check if the entire spec is a palette key
    if let Some(resolved) = palette.get(spec) {
        return parse_color_spec_raw(resolved);
    }

    parse_color_spec_raw(spec)
}

fn parse_color_spec_raw(spec: &str) -> ColorSpec {
    let spec = spec.trim();
    if spec.is_empty() || spec == "default" || spec == "none" {
        return ColorSpec::Ansi(b"\x1b[0m".to_vec());
    }
    if spec == "unchanged" {
        return ColorSpec::Unchanged;
    }
    if spec == "previous" || spec == "prev" {
        return ColorSpec::Previous;
    }

    let mut buf = Vec::with_capacity(16);
    for part in spec.split_whitespace() {
        if let Some(code) = ansi_code(part) {
            buf.extend_from_slice(code);
        } else if let Some(code) = parse_extended_color(part) {
            buf.extend_from_slice(&code);
        }
    }
    if buf.is_empty() {
        ColorSpec::Ansi(b"\x1b[0m".to_vec())
    } else {
        ColorSpec::Ansi(buf)
    }
}

/// Parse 256-color and RGB color tokens.
/// Supports: color256(N), on_color256(N), rgb(R,G,B), on_rgb(R,G,B)
fn parse_extended_color(token: &str) -> Option<Vec<u8>> {
    // color256(N) → \x1b[38;5;Nm
    if let Some(inner) = token.strip_prefix("color256(").and_then(|s| s.strip_suffix(')')) {
        let n: u8 = inner.trim().parse().ok()?;
        return Some(format!("\x1b[38;5;{n}m").into_bytes());
    }
    // on_color256(N) → \x1b[48;5;Nm
    if let Some(inner) = token.strip_prefix("on_color256(").and_then(|s| s.strip_suffix(')')) {
        let n: u8 = inner.trim().parse().ok()?;
        return Some(format!("\x1b[48;5;{n}m").into_bytes());
    }
    // rgb(R,G,B) → \x1b[38;2;R;G;Bm
    if let Some(inner) = token.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            let r: u8 = parts[0].trim().parse().ok()?;
            let g: u8 = parts[1].trim().parse().ok()?;
            let b: u8 = parts[2].trim().parse().ok()?;
            return Some(format!("\x1b[38;2;{r};{g};{b}m").into_bytes());
        }
    }
    // on_rgb(R,G,B) → \x1b[48;2;R;G;Bm
    if let Some(inner) = token.strip_prefix("on_rgb(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        if parts.len() == 3 {
            let r: u8 = parts[0].trim().parse().ok()?;
            let g: u8 = parts[1].trim().parse().ok()?;
            let b: u8 = parts[2].trim().parse().ok()?;
            return Some(format!("\x1b[48;2;{r};{g};{b}m").into_bytes());
        }
    }
    None
}

/// Map a single color token to its ANSI escape sequence (fast path).
fn ansi_code(name: &str) -> Option<&'static [u8]> {
    Some(match name {
        "none" | "default" | "reset" => b"\x1b[0m",
        "bold" => b"\x1b[1m",
        "dim" | "dark" => b"\x1b[2m",
        "italic" => b"\x1b[3m",
        "underline" => b"\x1b[4m",
        "blink" => b"\x1b[5m",
        "rapidblink" => b"\x1b[6m",
        "reverse" => b"\x1b[7m",
        "concealed" | "hidden" => b"\x1b[8m",
        "strikethrough" => b"\x1b[9m",
        "black" => b"\x1b[30m",
        "red" => b"\x1b[31m",
        "green" => b"\x1b[32m",
        "yellow" => b"\x1b[33m",
        "blue" => b"\x1b[34m",
        "magenta" => b"\x1b[35m",
        "cyan" => b"\x1b[36m",
        "white" => b"\x1b[37m",
        "bright_black" => b"\x1b[90m",
        "bright_red" => b"\x1b[91m",
        "bright_green" => b"\x1b[92m",
        "bright_yellow" => b"\x1b[93m",
        "bright_blue" => b"\x1b[94m",
        "bright_magenta" => b"\x1b[95m",
        "bright_cyan" => b"\x1b[96m",
        "bright_white" => b"\x1b[97m",
        "on_black" => b"\x1b[40m",
        "on_red" => b"\x1b[41m",
        "on_green" => b"\x1b[42m",
        "on_yellow" => b"\x1b[43m",
        "on_blue" => b"\x1b[44m",
        "on_magenta" => b"\x1b[45m",
        "on_cyan" => b"\x1b[46m",
        "on_white" => b"\x1b[47m",
        "on_bright_black" => b"\x1b[100m",
        "on_bright_red" => b"\x1b[101m",
        "on_bright_green" => b"\x1b[102m",
        "on_bright_yellow" => b"\x1b[103m",
        "on_bright_blue" => b"\x1b[104m",
        "on_bright_magenta" => b"\x1b[105m",
        "on_bright_cyan" => b"\x1b[106m",
        "on_bright_white" => b"\x1b[107m",
        _ => return None,
    })
}

pub const RESET: &[u8] = b"\x1b[0m";
