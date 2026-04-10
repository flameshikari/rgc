// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Self-update: downloads the latest release from GitHub and replaces the
//! running binary. Requires `curl` to be available in PATH.

use std::fs;
use std::io::Write;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const REPO: &str = "flameshikari/rgc";
const VERSION: &str = env!("CARGO_PKG_VERSION");

const B: &str = "\x1b[1m";
const D: &str = "\x1b[2m";
const R: &str = "\x1b[0m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";
const RED: &str = "\x1b[31m";
const MAGENTA: &str = "\x1b[35m";

const SPINNER: &[&str] = &[
    "\x1b[36m\u{28f7}\x1b[0m",
    "\x1b[36m\u{28ef}\x1b[0m",
    "\x1b[36m\u{28df}\x1b[0m",
    "\x1b[36m\u{287f}\x1b[0m",
    "\x1b[36m\u{28bf}\x1b[0m",
    "\x1b[36m\u{28fb}\x1b[0m",
    "\x1b[36m\u{28fd}\x1b[0m",
    "\x1b[36m\u{28fe}\x1b[0m",
];

static STOP: AtomicBool = AtomicBool::new(false);

fn spinner_start(msg: &str) -> std::thread::JoinHandle<()> {
    STOP.store(false, Ordering::Relaxed);
    let msg = msg.to_string();
    std::thread::spawn(move || {
        let mut i = 0;
        let mut err = std::io::stderr();
        while !STOP.load(Ordering::Relaxed) {
            let _ = write!(err, "\r{} {msg} ", SPINNER[i % SPINNER.len()]);
            let _ = err.flush();
            i += 1;
            std::thread::sleep(Duration::from_millis(80));
        }
        let _ = write!(err, "\r\x1b[2K");
        let _ = err.flush();
    })
}

fn spinner_stop(handle: std::thread::JoinHandle<()>) {
    STOP.store(true, Ordering::Relaxed);
    let _ = handle.join();
}

fn die(msg: &str) -> ! {
    eprintln!("{RED}X{R} {msg}");
    std::process::exit(1);
}

pub fn run() {
    let arch = if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else {
        die("unsupported architecture for self-update");
    };

    // --- Check ---
    let sp = spinner_start("Checking for updates...");

    let output = Command::new("curl")
        .args([
            "-sfL",
            "-H",
            "Accept: application/vnd.github+json",
            &format!("https://api.github.com/repos/{REPO}/releases/latest"),
        ])
        .output();

    spinner_stop(sp);

    let output = match output {
        Ok(o) if o.status.success() => o,
        Ok(_) => die("no releases found or GitHub API rate limit exceeded"),
        Err(_) => die("failed to check for updates (is curl installed?)"),
    };

    let release: serde_json::Value = match serde_json::from_slice(&output.stdout) {
        Ok(v) => v,
        Err(_) => die("failed to parse release info"),
    };

    let Some(tag) = release["tag_name"].as_str() else {
        die("no tag found in latest release");
    };

    let latest = tag.strip_prefix('v').unwrap_or(tag);

    if latest == VERSION {
        eprintln!("{GREEN}V{R} Already up to date: {D}{VERSION}{R}");
        return;
    }

    // --- Find asset ---
    let asset_name = format!("rgc-{tag}-{arch}.tar.gz");
    let Some(url) = release["assets"].as_array().and_then(|assets| {
        assets.iter().find_map(|a| {
            (a["name"].as_str() == Some(asset_name.as_str()))
                .then(|| a["browser_download_url"].as_str())
                .flatten()
        })
    }) else {
        die(&format!("no {arch} binary in release {tag}"));
    };

    let exe = match std::env::current_exe().and_then(fs::canonicalize) {
        Ok(p) => p,
        Err(e) => die(&format!("cannot locate binary: {e}")),
    };

    let tmp = exe.with_extension("tmp");

    // --- Download ---
    let sp = spinner_start(&format!("Downloading {MAGENTA}{asset_name}{R}..."));

    let downloaded = Command::new("sh")
        .arg("-c")
        .arg(format!(
            "curl -sfL '{url}' | tar xzf - -O > '{}'",
            tmp.display()
        ))
        .status()
        .is_ok_and(|s| s.success());

    spinner_stop(sp);

    if !downloaded {
        let _ = fs::remove_file(&tmp);
        die("download failed");
    }

    eprintln!("{GREEN}V{R} Downloaded {MAGENTA}{asset_name}{R}!");

    // --- Verify ---
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755));
    }

    let new_version = match Command::new(&tmp).arg("-v").output() {
        Ok(o) if o.status.success() => {
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        }
        _ => {
            let _ = fs::remove_file(&tmp);
            die("downloaded binary verification failed");
        }
    };

    // --- Install ---
    let sp = spinner_start("Installing...");

    if let Err(e) = fs::rename(&tmp, &exe) {
        spinner_stop(sp);
        let _ = fs::remove_file(&tmp);
        if e.kind() == std::io::ErrorKind::PermissionDenied {
            die(&format!(
                "{RED}Permission denied{R} -- try: {B}sudo rgc{R} {CYAN}--update{R}"
            ));
        } else {
            die(&format!("failed to replace binary: {e}"));
        }
    }

    spinner_stop(sp);

    // --- Done ---
    let installed = Command::new(&exe)
        .arg("-v")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or(new_version);

    eprintln!(
        "{GREEN}V{R} Updated: {D}{VERSION}{R} -> {D}{installed}{R} ({MAGENTA}{}{R})",
        exe.display(),
    );
}
