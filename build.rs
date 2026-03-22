// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Build script: embeds YAML config files into the binary via `include_str!`,
//! and pre-builds the command -> config name mapping at compile time.

use std::fs;
use std::io::Write;
use std::path::Path;

fn main() {
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let dest = Path::new(&out_dir).join("embedded.rs");
    let mut f = fs::File::create(&dest).unwrap();

    let mut entries: Vec<_> = fs::read_dir("configs")
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            e.file_name()
                .to_str()
                .is_some_and(|n| n.ends_with(".yaml"))
        })
        .collect();
    entries.sort_by_key(|e| e.file_name());

    // Embed config contents
    writeln!(f, "pub const CONFIGS: &[(&str, &str)] = &[").unwrap();
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_str().unwrap();
        let base = name.strip_suffix(".yaml").unwrap_or(name);
        writeln!(
            f,
            "    (\"{base}\", include_str!(concat!(env!(\"CARGO_MANIFEST_DIR\"), \"/configs/{name}\"))),",
        )
        .unwrap();
    }
    writeln!(f, "];").unwrap();

    // Pre-build the command -> config name map by parsing YAML headers at build time.
    // This eliminates ~1-3 ms of YAML parsing on every program startup.
    writeln!(f).unwrap();
    writeln!(f, "pub const COMMAND_MAP_ENTRIES: &[(&str, &str)] = &[").unwrap();
    for entry in &entries {
        let name = entry.file_name();
        let name = name.to_str().unwrap();
        let base = name.strip_suffix(".yaml").unwrap_or(name);
        let content = fs::read_to_string(entry.path()).unwrap();
        for cmd in extract_commands(&content) {
            let escaped = cmd.replace('\\', "\\\\").replace('"', "\\\"");
            writeln!(f, "    (\"{escaped}\", \"{base}\"),").unwrap();
        }
    }
    writeln!(f, "];").unwrap();

    println!("cargo:rerun-if-changed=configs");
    println!("cargo:rerun-if-changed=build.rs");

    // Link libutil for openpty()
    #[cfg(target_os = "linux")]
    println!("cargo:rustc-link-lib=util");
}

/// Tiny YAML parser that extracts the items of a top-level `command:` list.
/// Only handles the block list format used by all bundled configs:
///
/// ```yaml
/// command:
///   - foo
///   - bar
/// ```
fn extract_commands(content: &str) -> Vec<String> {
    let mut commands = Vec::new();
    let mut in_section = false;

    for line in content.lines() {
        if !in_section {
            if let Some(rest) = line.strip_prefix("command:") {
                let rest = rest.trim();
                if !rest.is_empty() && !rest.starts_with('#') {
                    panic!("inline command list not supported, use block list format");
                }
                in_section = true;
            }
            continue;
        }

        let trimmed = line.trim_start();

        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        if let Some(item) = trimmed.strip_prefix("- ") {
            let item = item.trim();
            // Strip surrounding quotes if any
            let item = if (item.starts_with('\'') && item.ends_with('\''))
                || (item.starts_with('"') && item.ends_with('"'))
            {
                &item[1..item.len() - 1]
            } else {
                item
            };
            commands.push(item.to_string());
        } else if !line.starts_with(' ') && !line.starts_with('\t') {
            // Non-indented line means we left the command section
            break;
        }
    }

    commands
}
