// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

mod color;
mod config;
mod early_detect;
mod engine;
mod process;
mod registry;
mod yaml_config;

use std::path::Path;

use is_terminal::IsTerminal;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() {
    // Handle SIGPIPE gracefully (don't print error when piped to head, etc.)
    #[cfg(unix)]
    unsafe {
        libc_sigpipe_default();
    }

    // Pipe peer was already detected pre-main via .init_array (early_detect.rs).
    // Fall back to runtime detection if pre-main didn't catch it.
    let pipe_peer_cmd = early_detect::get_early_peer_cmd().or_else(|| {
        #[cfg(target_os = "linux")]
        {
            detect_pipe_peer()
        }
        #[cfg(not(target_os = "linux"))]
        {
            None
        }
    });

    let args: Vec<String> = std::env::args().skip(1).collect();

    if args.is_empty() && std::io::stdin().is_terminal() {
        print_usage();
        std::process::exit(0);
    }

    // Parse rgc options
    let mut config_name: Option<String> = None;
    let mut config_dir: Option<String> = None;
    let mut color_mode = ColorMode::Auto;
    let mut command_args: Vec<String> = Vec::new();
    let mut i = 0;

    while i < args.len() {
        match args[i].as_str() {
            "-h" | "--help" => {
                print_usage();
                std::process::exit(0);
            }
            "-v" | "-V" | "--version" => {
                println!("{VERSION}");
                std::process::exit(0);
            }
            "-l" | "--list" => {
                print_supported_commands();
                std::process::exit(0);
            }
            "-a" | "--aliases" => {
                print_aliases();
                std::process::exit(0);
            }
            "-c" | "--config" => {
                i += 1;
                if i < args.len() {
                    config_name = Some(args[i].clone());
                } else {
                    eprintln!("rgc: --config requires a value");
                    std::process::exit(1);
                }
            }
            "-d" | "--config-dir" => {
                i += 1;
                if i < args.len() {
                    config_dir = Some(args[i].clone());
                } else {
                    eprintln!("rgc: --config-dir requires a path");
                    std::process::exit(1);
                }
            }
            "--colour" | "--color" => {
                i += 1;
                if i < args.len() {
                    color_mode = match args[i].as_str() {
                        "on" | "always" => ColorMode::On,
                        "off" | "never" => ColorMode::Off,
                        "auto" => ColorMode::Auto,
                        _ => {
                            eprintln!("rgc: unknown color mode: {}", args[i]);
                            std::process::exit(1);
                        }
                    };
                }
            }
            s if s.starts_with("--colour=") || s.starts_with("--color=") => {
                let val = s.split_once('=').unwrap().1;
                color_mode = match val {
                    "on" | "always" => ColorMode::On,
                    "off" | "never" => ColorMode::Off,
                    "auto" => ColorMode::Auto,
                    _ => {
                        eprintln!("rgc: unknown color mode: {val}");
                        std::process::exit(1);
                    }
                };
            }
            _ => {
                // Everything from here on is the command (or pipe hint)
                command_args = args[i..].to_vec();
                break;
            }
        }
        i += 1;
    }

    // Determine if we should colorize
    // --color on/off: explicit override, always wins
    // --color auto (default): FORCE_COLOR forces on, NO_COLOR forces off, then check TTY
    let should_color = match color_mode {
        ColorMode::On => true,
        ColorMode::Off => false,
        ColorMode::Auto => {
            if std::env::var_os("FORCE_COLOR").is_some() {
                true
            } else if std::env::var_os("NO_COLOR").is_some() {
                false
            } else {
                std::io::stdout().is_terminal()
            }
        }
    };

    // Determine mode:
    // - Pipe mode: stdin is a pipe/fifo (data is being piped in)
    // - Direct mode: stdin is a terminal, /dev/null, or anything else + we have command args
    let pipe_mode = stdin_is_pipe();

    if !should_color {
        if !pipe_mode && !command_args.is_empty() {
            let status = std::process::Command::new(&command_args[0])
                .args(&command_args[1..])
                .status()
                .unwrap_or_else(|e| {
                    eprintln!("rgc: {}: {}", command_args[0], e);
                    std::process::exit(127);
                });
            std::process::exit(status.code().unwrap_or(1));
        } else {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::stdout().lock());
            std::process::exit(0);
        }
    }

    // Resolve which config to use. Returns (config_name, matched_command).
    let resolved = resolve_config(
        &config_name,
        &command_args,
        pipe_mode,
        &config_dir,
        &pipe_peer_cmd,
    );

    let Some((config_name, matched_cmd)) = resolved else {
        if command_args.is_empty() {
            if pipe_mode {
                let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::stdout().lock());
                std::process::exit(0);
            }
            eprintln!("rgc: no config found. Use -c <config> or specify a command.");
            std::process::exit(1);
        }
        // No config found — run command without colorization
        if !pipe_mode {
            let status = std::process::Command::new(&command_args[0])
                .args(&command_args[1..])
                .status()
                .unwrap_or_else(|e| {
                    eprintln!("rgc: {}: {}", command_args[0], e);
                    std::process::exit(127);
                });
            std::process::exit(status.code().unwrap_or(1));
        } else {
            let _ = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::stdout().lock());
            std::process::exit(0);
        }
    };

    // Parse and compile rules — from external dir or bundled
    let matched_ref = matched_cmd.as_deref();
    let rules = if let Some(ref dir) = config_dir {
        registry::get_rules_from_dir(&config_name, dir, matched_ref).unwrap_or_else(|| {
            eprintln!("rgc: config not found: {dir}/{config_name}");
            std::process::exit(1);
        })
    } else {
        registry::get_rules(&config_name, matched_ref).unwrap_or_else(|| {
            eprintln!("rgc: unknown config: {config_name}");
            std::process::exit(1);
        })
    };

    // Dispatch based on mode
    if pipe_mode {
        process::run_pipe(&rules);
    } else if !command_args.is_empty() {
        let exit_code = process::run_command(&command_args, &rules);
        std::process::exit(exit_code);
    } else {
        eprintln!("rgc: no command specified");
        std::process::exit(1);
    }
}

#[derive(Clone, Copy)]
enum ColorMode {
    Auto,
    On,
    Off,
}

/// Check if stdin is a pipe/fifo (data is being piped in).
#[cfg(unix)]
fn stdin_is_pipe() -> bool {
    use std::os::unix::io::AsRawFd;
    let fd = std::io::stdin().as_raw_fd();
    unsafe {
        let mut stat: libc::stat_t = std::mem::zeroed();
        if libc::fstat(fd, &mut stat) == 0 {
            // S_IFIFO = 0o010000
            (stat.st_mode & 0o170000) == 0o010000
        } else {
            // Fallback: not a terminal means pipe
            !std::io::stdin().is_terminal()
        }
    }
}

#[cfg(not(unix))]
fn stdin_is_pipe() -> bool {
    !std::io::stdin().is_terminal()
}

/// Returns (config_name, matched_command) where matched_command is the full
/// command string that was matched (e.g., "docker ps") for `when` filtering.
fn resolve_config(
    explicit: &Option<String>,
    command_args: &[String],
    pipe_mode: bool,
    config_dir: &Option<String>,
    pipe_peer_cmd: &Option<String>,
) -> Option<(String, Option<String>)> {
    // 1. Explicit -c flag — no matched command (all rules apply)
    if let Some(name) = explicit {
        return Some((name.clone(), None));
    }

    // 2. Match command from args
    if !command_args.is_empty()
        && let Some((conf, matched)) = lookup_from_args(command_args, config_dir)
    {
        return Some((conf, Some(matched)));
    }

    // 3. In pipe mode, use pre-captured pipe peer command
    if pipe_mode && command_args.is_empty()
        && let Some(peer_cmd) = pipe_peer_cmd
    {
        let parts: Vec<String> =
            peer_cmd.split_whitespace().map(|s| s.to_string()).collect();
        if !parts.is_empty()
            && let Some((conf, matched)) = lookup_from_args(&parts, config_dir)
        {
            return Some((conf, Some(matched)));
        }
    }

    None
}

/// Returns (config_name, matched_command_string).
fn lookup_from_args(args: &[String], config_dir: &Option<String>) -> Option<(String, String)> {
    if args.is_empty() {
        return None;
    }

    let real_args = strip_wrappers(args);

    if !real_args.is_empty() && real_args.len() < args.len() {
        return do_lookup(real_args, config_dir);
    }

    do_lookup(args, config_dir)
}

fn do_lookup(args: &[String], config_dir: &Option<String>) -> Option<(String, String)> {
    let cmd = extract_command_name(&args[0]);
    let sub_args = &args[1..];

    if let Some(dir) = config_dir {
        let config = registry::lookup_config_from_dir(cmd, sub_args, dir)?;
        // Build matched command from args
        let matched = if sub_args.is_empty() {
            cmd.to_string()
        } else {
            format!("{cmd} {}", sub_args.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(" "))
        };
        Some((config, matched))
    } else {
        let (config, matched_key) = registry::lookup_config(cmd, sub_args)?;
        Some((config.to_string(), matched_key))
    }
}

/// Extract the base command name from a potentially full path.
fn extract_command_name(cmd: &str) -> &str {
    Path::new(cmd)
        .file_name()
        .and_then(|f| f.to_str())
        .unwrap_or(cmd)
}

/// Commands that wrap other commands — skip these to find the real command.
const WRAPPER_COMMANDS: &[&str] = &[
    "sudo", "env", "nice", "nohup", "stdbuf", "unbuffer", "time", "timeout", "watch",
    "ionice", "chrt", "taskset", "setsid", "runuser", "su",
];

/// Strip wrapper commands and their flags from args to find the real command.
/// e.g., `["sudo", "-u", "root", "tcpdump", "-i", "eth0"]` -> `["tcpdump", "-i", "eth0"]`.
fn strip_wrappers(args: &[String]) -> &[String] {
    let mut i = 0;
    while i < args.len() {
        let cmd = extract_command_name(&args[i]);
        if !WRAPPER_COMMANDS.contains(&cmd) {
            return &args[i..];
        }
        i += 1;
        // Skip flags belonging to the wrapper (e.g., sudo -u root, env -i, timeout 5)
        while i < args.len() && args[i].starts_with('-') {
            i += 1;
            // Some flags take a value (sudo -u root, timeout --signal=KILL)
            // Skip the value if the flag doesn't contain '='
            if i > 0
                && !args[i - 1].contains('=')
                && i < args.len()
                && !args[i].starts_with('-')
            {
                // Peek: if next arg looks like a flag value (not a command), skip it
                let next = &args[i];
                if !next.contains('/') && next.chars().next().is_some_and(|c| !c.is_alphabetic()) {
                    i += 1;
                }
            }
        }
        // Also skip bare numeric args for wrappers like timeout (e.g., "timeout 5 cmd")
        if i < args.len() && args[i].parse::<f64>().is_ok() {
            i += 1;
        }
    }
    &args[args.len()..] // all wrappers, no real command found
}

/// Detect the command on the other end of the pipe by scanning process group siblings.
/// In a shell pipeline like `sudo tcpdump | rgc`, all processes share the same PGID.
/// /proc/<pid>/stat and /proc/<pid>/cmdline are world-readable, so this works
/// even when the peer runs as root (e.g., sudo).
#[cfg(target_os = "linux")]
fn detect_pipe_peer() -> Option<String> {
    use std::fs;

    let my_pid = std::process::id();
    let my_pgid = read_pgid(my_pid)?;
    let my_ppid = std::os::unix::process::parent_id();

    // Scan /proc for processes in the same process group
    let proc_dir = fs::read_dir("/proc").ok()?;
    for entry in proc_dir {
        let Ok(entry) = entry else { continue };
        let name = entry.file_name();
        let Some(pid_str) = name.to_str() else { continue };
        let Ok(pid) = pid_str.parse::<u32>() else { continue };

        // Skip ourselves and the parent shell
        if pid == my_pid || pid == my_ppid {
            continue;
        }

        // Check if this process is in our process group
        let Some(pgid) = read_pgid(pid) else { continue };
        if pgid != my_pgid {
            continue;
        }

        // Found a sibling — read its cmdline
        if let Ok(cmdline) = fs::read_to_string(format!("/proc/{pid}/cmdline")) {
            let cmd: String = cmdline
                .split('\0')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !cmd.is_empty() {
                return Some(cmd);
            }
        }

        // Fallback: /proc/<pid>/stat has the comm name (truncated to 15 chars)
        if let Some(comm) = read_comm(pid) {
            return Some(comm);
        }
    }

    None
}

/// Read the process group ID from /proc/<pid>/stat.
/// Format: pid (comm) state ppid pgrp ...
#[cfg(target_os = "linux")]
fn read_pgid(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Find the closing ')' of comm field, then parse fields after it
    let after_comm = stat.rsplit_once(')')?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // fields[0]=state, fields[1]=ppid, fields[2]=pgrp
    fields.get(2)?.parse().ok()
}

/// Read the command name from /proc/<pid>/stat (truncated to 15 chars).
#[cfg(target_os = "linux")]
fn read_comm(pid: u32) -> Option<String> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let start = stat.find('(')? + 1;
    let end = stat.rfind(')')?;
    let comm = &stat[start..end];
    if comm.is_empty() {
        None
    } else {
        Some(comm.to_string())
    }
}

fn help_colors_enabled() -> bool {
    if std::env::var_os("FORCE_COLOR").is_some() {
        return true;
    }
    if std::env::var_os("NO_COLOR").is_some() {
        return false;
    }
    std::io::stdout().is_terminal()
}

fn print_usage() {
    let colored = help_colors_enabled();
    let g = if colored { "\x1b[32m" } else { "" };
    let c = if colored { "\x1b[36m" } else { "" };
    let y = if colored { "\x1b[33m" } else { "" };
    let b = if colored { "\x1b[1m" } else { "" };
    let d = if colored { "\x1b[2m" } else { "" };
    let r = if colored { "\x1b[0m" } else { "" };

    println!(
        "{b}{g}rgc{r} {d}{VERSION}{r} - Rust Generic Colorizer

{b}{y}Usage:{r}
  {b}rgc{r} {c}[OPTIONS]{r} {g}COMMAND [ARGS...]{r}  Run command with colorized output
  {g}COMMAND{r} | {b}rgc{r} {c}[OPTIONS]{r}          Colorize piped input

{b}{y}Options:{r}
  {c}-c{r}, {c}--config{r} {d}<NAME>{r}        Use specific config {d}(e.g., ping or docker){r}
  {c}-d{r}, {c}--config-dir{r} {d}<PATH>{r}    Load configs from directory instead of bundled ones
      {c}--color{r} {d}<on|off|auto>{r}  Control colorization {d}(default: auto){r}
  {c}-l{r}, {c}--list{r}                 List supported commands
  {c}-a{r}, {c}--aliases{r}              Generate shell aliases for .bashrc/.zshrc
  {c}-h{r}, {c}--help{r}                 Show this help
  {c}-v{r}, {c}--version{r}              Show version

{b}{y}Examples:{r}
  {b}rgc{r} {g}ping 8.8.8.8{r}   Colorize ping output
  {b}rgc{r} {g}docker ps{r}      Colorize docker ps
  {g}mount{r} | {b}rgc{r}        Auto-detect and colorize
  {g}df -h{r} | {b}rgc{r} {c}-c{r} {d}df{r}  Explicit config in pipe mode
  source <({b}rgc{r} {c}-a{r})   Load aliases into current shell"
    );
}

fn print_supported_commands() {
    let commands = registry::all_commands();
    println!("Supported commands ({}):\n", commands.len());
    for cmd in &commands {
        println!("  {cmd}");
    }
}

fn print_aliases() {
    use std::collections::BTreeSet;

    let commands = registry::all_commands();

    let rgc_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "rgc".to_string());

    // Collect all base commands to alias (deduplicated, sorted)
    let mut aliases = BTreeSet::new();
    for cmd in &commands {
        // For compound commands like "docker ps", alias the base command "docker"
        let base = cmd.split_whitespace().next().unwrap_or(cmd);
        // Skip single-char commands like "w" to avoid breaking shell builtins
        if base.len() > 1 {
            aliases.insert(base);
        }
    }

    println!("# rgc aliases — add to .bashrc or .zshrc");
    println!("# Generated by: rgc --aliases\n");

    for cmd in &aliases {
        println!("alias {cmd}='{rgc_path} {cmd}'");
    }
}

/// Reset SIGPIPE to default behavior (terminate process) so piping to `head` etc. works.
#[cfg(unix)]
unsafe fn libc_sigpipe_default() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

#[cfg(unix)]
mod libc {
    unsafe extern "C" {
        pub fn signal(signum: i32, handler: usize) -> usize;
        pub fn fstat(fd: i32, buf: *mut stat_t) -> i32;
    }
    pub const SIGPIPE: i32 = 13;
    pub const SIG_DFL: usize = 0;

    #[repr(C)]
    #[allow(non_camel_case_types)]
    pub struct stat_t {
        pub st_dev: u64,
        pub st_ino: u64,
        pub st_nlink: u64,
        pub st_mode: u32,
        pub st_uid: u32,
        pub st_gid: u32,
        _pad0: u32,
        pub st_rdev: u64,
        pub st_size: i64,
        pub st_blksize: i64,
        pub st_blocks: i64,
        pub st_atime: i64,
        pub st_atime_nsec: i64,
        pub st_mtime: i64,
        pub st_mtime_nsec: i64,
        pub st_ctime: i64,
        pub st_ctime_nsec: i64,
        _unused: [i64; 3],
    }
}
