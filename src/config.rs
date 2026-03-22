// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Core types shared across the crate: compiled regexes, count modes, and colorization rules.

use crate::color::ColorSpec;

/// A compiled regex that uses the fast `regex` crate when possible,
/// falling back to `fancy_regex` for patterns with lookahead/lookbehind.
#[derive(Debug)]
pub enum CompiledRegex {
    Fast(regex::Regex),
    Fancy(fancy_regex::Regex),
}

impl CompiledRegex {
    /// Compile a pattern, trying the SIMD-accelerated `regex` crate first.
    pub fn new(pattern: &str) -> Option<Self> {
        match regex::Regex::new(pattern) {
            Ok(re) => Some(CompiledRegex::Fast(re)),
            Err(_) => match fancy_regex::Regex::new(pattern) {
                Ok(re) => Some(CompiledRegex::Fancy(re)),
                Err(_) => None,
            },
        }
    }

    /// Get named capture group indices: name -> group index.
    pub fn capture_names(&self) -> Vec<(String, usize)> {
        match self {
            CompiledRegex::Fast(re) => re
                .capture_names()
                .enumerate()
                .skip(1)
                .filter_map(|(i, name)| name.map(|n| (n.to_string(), i)))
                .collect(),
            CompiledRegex::Fancy(re) => re
                .capture_names()
                .enumerate()
                .skip(1)
                .filter_map(|(i, name)| name.map(|n| (n.to_string(), i)))
                .collect(),
        }
    }
}

/// How many times a rule can fire per line.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CountMode {
    More,
    Once,
    Stop,
    Previous,
    Block,
    Unblock,
}

/// A single colorization rule (compiled and ready to apply).
#[derive(Debug)]
pub struct Rule {
    pub regex: CompiledRegex,
    pub colors: Vec<ColorSpec>,
    pub count: CountMode,
    pub skip: bool,
    pub replace: Option<String>,
}
