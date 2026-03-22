// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Config registry: maps command names to bundled YAML configs via O(1) HashMap lookup.
//!
//! The map is pre-built by `build.rs` at compile time (see `COMMAND_MAP_ENTRIES`),
//! so program startup pays no YAML parsing cost for the command lookup.

use std::collections::HashMap;
use std::sync::LazyLock;

use crate::config::Rule;
use crate::yaml_config;

include!(concat!(env!("OUT_DIR"), "/embedded.rs"));

/// Command string -> config name. Built once from the compile-time
/// `COMMAND_MAP_ENTRIES` slice on first lookup.
static COMMAND_MAP: LazyLock<HashMap<&'static str, &'static str>> =
    LazyLock::new(|| COMMAND_MAP_ENTRIES.iter().copied().collect());

/// Look up config name by command + args. O(1) HashMap lookup.
/// Returns (config_name, matched_command_key).
pub fn lookup_config(cmd: &str, args: &[String]) -> Option<(&'static str, String)> {
    let map = &*COMMAND_MAP;

    if !args.is_empty() {
        let mut key_buf = [0u8; 128];
        if args.len() >= 2
            && let Some(key) = build_key(&mut key_buf, cmd, &args[0], Some(&args[1]))
            && let Some(conf) = map.get(key)
        {
            return Some((conf, key.to_string()));
        }
        if let Some(key) = build_key(&mut key_buf, cmd, &args[0], None)
            && let Some(conf) = map.get(key)
        {
            return Some((conf, key.to_string()));
        }
    }

    map.get(cmd).map(|c| (*c, cmd.to_string()))
}

/// Return all registered command strings, sorted alphabetically.
pub fn all_commands() -> Vec<&'static str> {
    let mut cmds: Vec<&str> = COMMAND_MAP_ENTRIES.iter().map(|(k, _)| *k).collect();
    cmds.sort();
    cmds.dedup();
    cmds
}

fn build_key<'a>(
    buf: &'a mut [u8; 128],
    cmd: &str,
    arg1: &str,
    arg2: Option<&str>,
) -> Option<&'a str> {
    let mut pos = 0;
    let parts: &[&str] = if let Some(a2) = arg2 {
        &[cmd, " ", arg1, " ", a2]
    } else {
        &[cmd, " ", arg1]
    };
    for part in parts {
        let b = part.as_bytes();
        if pos + b.len() > buf.len() {
            return None;
        }
        buf[pos..pos + b.len()].copy_from_slice(b);
        pos += b.len();
    }
    std::str::from_utf8(&buf[..pos]).ok()
}

/// Parse and compile rules for a single bundled config.
/// If `matched_cmd` is provided, skips rules that don't match the `when` filter
/// (avoids compiling their regexes).
pub fn get_rules(config_name: &str, matched_cmd: Option<&str>) -> Option<Vec<Rule>> {
    let content = CONFIGS
        .iter()
        .find(|&&(n, _)| n == config_name)
        .map(|&(_, content)| content)?;
    let (_, rules) = yaml_config::parse_yaml_config(content, matched_cmd)?;
    Some(rules)
}

/// Parse and compile rules from an external config directory.
pub fn get_rules_from_dir(config_name: &str, dir: &str, matched_cmd: Option<&str>) -> Option<Vec<Rule>> {
    let yaml_path = std::path::Path::new(dir).join(format!("{config_name}.yaml"));
    let plain_path = std::path::Path::new(dir).join(config_name);
    let content = std::fs::read_to_string(&yaml_path)
        .or_else(|_| std::fs::read_to_string(&plain_path))
        .ok()?;
    let (_, rules) = yaml_config::parse_yaml_config(&content, matched_cmd)?;
    Some(rules)
}

/// Look up config from an external directory by scanning YAML command fields.
pub fn lookup_config_from_dir(cmd: &str, args: &[String], dir: &str) -> Option<String> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        if !name_str.ends_with(".yaml") {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        let Some(commands) = yaml_config::parse_yaml_commands(&content) else {
            continue;
        };

        let config_name = name_str.strip_suffix(".yaml").unwrap_or(name_str);

        for header_cmd in &commands {
            if !args.is_empty() {
                if args.len() >= 2 {
                    let key = format!("{cmd} {} {}", args[0], args[1]);
                    if *header_cmd == key {
                        return Some(config_name.to_string());
                    }
                }
                let key = format!("{cmd} {}", args[0]);
                if *header_cmd == key {
                    return Some(config_name.to_string());
                }
            }
            if *header_cmd == cmd {
                return Some(config_name.to_string());
            }
        }
    }
    None
}
