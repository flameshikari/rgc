// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! YAML config parser: deserializes config files into compiled [`Rule`]s.

use std::collections::HashMap;

use serde::Deserialize;

use crate::color::{parse_color_spec_with_palette, ColorSpec};
use crate::config::{CompiledRegex, CountMode, Rule};

/// Top-level YAML config structure.
#[derive(Deserialize)]
pub struct YamlConfig {
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub palette: HashMap<String, String>,
    #[serde(default)]
    pub rules: Vec<YamlRule>,
}

/// Lightweight header-only deserialization (skips rules).
#[derive(Deserialize)]
pub struct YamlHeader {
    #[serde(default)]
    pub command: Vec<String>,
}

/// A single rule as defined in YAML (before compilation).
#[derive(Deserialize)]
pub struct YamlRule {
    /// Documentation-only field in YAML configs; not used at runtime.
    #[serde(default)]
    #[allow(dead_code)]
    pub name: Option<String>,
    #[serde(default)]
    pub when: Option<Vec<String>>,
    pub pattern: String,
    #[serde(default)]
    pub colors: ColorsDef,
    #[serde(default = "default_count")]
    pub count: String,
    #[serde(default)]
    pub skip: bool,
    #[serde(default)]
    pub replace: Option<String>,
}

fn default_count() -> String {
    "more".to_string()
}

/// Colors can be a positional list or a named map keyed by capture group name.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ColorsDef {
    #[default]
    None,
    Positional(Vec<String>),
    Named(HashMap<String, String>),
}

/// Parse a YAML config string into (commands, compiled rules).
/// If `matched_cmd` is provided, skips compiling rules that don't match the `when` filter.
pub fn parse_yaml_config(content: &str, matched_cmd: Option<&str>) -> Option<(Vec<String>, Vec<Rule>)> {
    let config: YamlConfig = serde_yml::from_str(content).ok()?;
    let palette = &config.palette;
    let mut rules = Vec::with_capacity(config.rules.len());

    for yaml_rule in &config.rules {
        // Skip rules that don't match the `when` filter BEFORE compiling regex
        if let (Some(when), Some(cmd)) = (&yaml_rule.when, matched_cmd)
            && !when.iter().any(|w| w == cmd)
        {
            continue;
        }

        let Some(regex) = CompiledRegex::new(&yaml_rule.pattern) else {
            eprintln!(
                "rgc: warning: failed to compile regex, skipping rule: {}",
                yaml_rule.pattern
            );
            continue;
        };

        let colors = match &yaml_rule.colors {
            ColorsDef::None => vec![],
            ColorsDef::Positional(list) => list
                .iter()
                .map(|s| parse_color_spec_with_palette(s, palette))
                .collect(),
            ColorsDef::Named(map) => resolve_named_colors(&regex, map, palette),
        };

        let count = match yaml_rule.count.as_str() {
            "once" => CountMode::Once,
            "stop" => CountMode::Stop,
            "previous" => CountMode::Previous,
            "block" => CountMode::Block,
            "unblock" => CountMode::Unblock,
            _ => CountMode::More,
        };

        rules.push(Rule {
            regex,
            colors,
            count,
            skip: yaml_rule.skip,
            replace: yaml_rule.replace.clone(),
        });
    }

    Some((config.command, rules))
}

/// Parse only the command list from a YAML config (fast, skips rules).
pub fn parse_yaml_commands(content: &str) -> Option<Vec<String>> {
    let header: YamlHeader = serde_yml::from_str(content).ok()?;
    Some(header.command)
}

/// Resolve named capture group colors to positional Vec<ColorSpec>.
fn resolve_named_colors(
    regex: &CompiledRegex,
    color_map: &HashMap<String, String>,
    palette: &HashMap<String, String>,
) -> Vec<ColorSpec> {
    let named_groups = regex.capture_names();
    if named_groups.is_empty() {
        return vec![];
    }

    // Find the highest group index to size the output
    let max_idx = named_groups.iter().map(|(_, idx)| *idx).max().unwrap_or(0);
    let mut colors = vec![ColorSpec::Unchanged; max_idx + 1];

    // Group 0 (whole match) defaults to unchanged
    colors[0] = ColorSpec::Unchanged;

    for (name, idx) in &named_groups {
        if let Some(color_str) = color_map.get(name) {
            colors[*idx] = parse_color_spec_with_palette(color_str, palette);
        }
    }

    colors
}
