// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Process execution: spawns commands with PTY or pipe and colorizes their output.

use std::io::{BufRead, BufReader, BufWriter, Write};
use std::os::unix::io::{FromRawFd, RawFd};
use std::process::{Command, Stdio};

use crate::config::Rule;
use crate::engine::Engine;

/// EIO — returned by PTY master read after the child process exits. Treated as EOF.
const EIO: i32 = 5;

/// Spawn a command with a PTY so it line-buffers its output, then colorize it.
pub fn run_command(args: &[String], rules: &[Rule]) -> i32 {
    match run_command_pty(args, rules) {
        Some(code) => code,
        None => run_command_pipe(args, rules),
    }
}

/// Spawn using a PTY master/slave pair.
fn run_command_pty(args: &[String], rules: &[Rule]) -> Option<i32> {
    let mut master: RawFd = -1;
    let mut slave: RawFd = -1;

    // Open a PTY pair
    let ret = unsafe {
        openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if ret != 0 {
        return None;
    }

    // Configure PTY: keep output processing (OPOST+ONLCR) so the child's
    // \n becomes \r\n like a normal terminal. Disable echo and canonical mode.
    // This makes the child see a normal terminal (line-buffers its output)
    // while we get clean \r\n-terminated lines on the master side.
    unsafe {
        let mut termios: libc_termios = std::mem::zeroed();
        tcgetattr(slave, &mut termios);
        // Disable input processing
        termios.c_iflag &=
            !(IGNBRK | BRKINT | PARMRK | ISTRIP | INLCR | IGNCR | ICRNL | IXON | IXOFF);
        // Keep OPOST (output processing) — child's \n → \r\n
        termios.c_oflag |= OPOST | ONLCR;
        // Disable echo, canonical mode, signals, extended processing
        termios.c_lflag &= !(ECHO | ECHONL | ICANON | ISIG | IEXTEN);
        // 8-bit clean
        termios.c_cflag &= !(CSIZE | PARENB);
        termios.c_cflag |= CS8;
        tcsetattr(slave, 0, &termios);

        // Set a reasonable window size
        let ws = Winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
        ioctl(slave, TIOCSWINSZ, &ws);
    }

    // Convert slave fd to Stdio for the child's stdout
    let slave_stdio = unsafe { Stdio::from_raw_fd(slave) };

    // Suppress the child's own coloring — we handle colorization.
    // TERM=dumb + NO_COLOR=1 prevents programs from emitting ANSI codes
    // that would interfere with our regex matching and color output.
    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .stdout(slave_stdio)
        .stderr(Stdio::inherit())
        .env("TERM", "dumb")
        .env("NO_COLOR", "1")
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rgc: {}: {}", args[0], e);
            unsafe {
                close(master);
            }
            return Some(127);
        }
    };

    // Read from the PTY master using raw reads + manual line splitting.
    // Some programs (especially under sudo) produce output that BufReader
    // doesn't split correctly on PTY fds.
    let master_file = unsafe { std::fs::File::from_raw_fd(master) };
    let out = std::io::stdout().lock();
    let mut writer = BufWriter::with_capacity(8192, out);
    let mut engine = Engine::new(rules);

    pty_read_loop(master_file, &mut writer, &mut engine);

    let _ = writer.flush();

    match child.wait() {
        Ok(status) => Some(status.code().unwrap_or(1)),
        Err(_) => Some(1),
    }
}

/// Fallback: spawn with a plain pipe (used if PTY fails).
fn run_command_pipe(args: &[String], rules: &[Rule]) -> i32 {
    let mut child = match Command::new(&args[0])
        .args(&args[1..])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("rgc: {}: {}", args[0], e);
            return 127;
        }
    };

    let stdout = child.stdout.take().unwrap();
    let reader = BufReader::with_capacity(8192, stdout);
    let out = std::io::stdout().lock();
    let mut writer = BufWriter::with_capacity(8192, out);

    colorize_stream(reader, &mut writer, rules, true);

    let _ = writer.flush();

    match child.wait() {
        Ok(status) => status.code().unwrap_or(1),
        Err(_) => 1,
    }
}

/// Read from stdin and colorize each line.
/// Flushes per line when stdout is a terminal (for streaming commands like ping).
pub fn run_pipe(rules: &[Rule]) {
    use is_terminal::IsTerminal;

    let line_flush = std::io::stdout().is_terminal();
    let stdin = std::io::stdin().lock();
    let reader = BufReader::with_capacity(if line_flush { 8192 } else { 65536 }, stdin);
    let out = std::io::stdout().lock();
    let mut writer = BufWriter::with_capacity(if line_flush { 8192 } else { 65536 }, out);

    colorize_stream(reader, &mut writer, rules, line_flush);

    let _ = writer.flush();
}

/// Read from PTY master using raw reads, split lines manually.
/// Works around PTY fd quirks that BufReader::read_line doesn't handle well.
fn pty_read_loop<W: Write>(mut master: std::fs::File, writer: &mut W, engine: &mut Engine) {
    use std::io::Read;
    let mut buf = [0u8; 8192];
    let mut line_buf = Vec::with_capacity(1024);

    loop {
        let n = match master.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                // EIO = child exited, normal for PTY
                if e.raw_os_error() == Some(EIO) {
                    break;
                }
                break;
            }
        };

        // Process the chunk byte by byte, splitting on \n.
        // \r is stripped in-place when it precedes \n (CRLF handling) so we
        // avoid an extra trim pass over the line buffer.
        for &byte in &buf[..n] {
            if byte == b'\n' {
                if line_buf.last() == Some(&b'\r') {
                    line_buf.pop();
                }
                let line = String::from_utf8_lossy(&line_buf);
                engine.colorize_line(&line, writer);
                let _ = writer.flush();
                line_buf.clear();
            } else {
                line_buf.push(byte);
            }
        }
    }

    // Flush any remaining partial line
    if !line_buf.is_empty() {
        if line_buf.last() == Some(&b'\r') {
            line_buf.pop();
        }
        let line = String::from_utf8_lossy(&line_buf);
        engine.colorize_line(&line, writer);
        let _ = writer.flush();
    }
}

/// Core streaming loop: read lines and colorize them.
fn colorize_stream<R: BufRead, W: Write>(
    reader: R,
    writer: &mut W,
    rules: &[Rule],
    line_flush: bool,
) {
    let mut engine = Engine::new(rules);
    colorize_stream_impl(reader, writer, &mut engine, line_flush);
}

fn colorize_stream_impl<R: BufRead, W: Write>(
    mut reader: R,
    writer: &mut W,
    engine: &mut Engine,
    line_flush: bool,
) {
    let mut line_buf = String::with_capacity(1024);

    loop {
        line_buf.clear();
        match reader.read_line(&mut line_buf) {
            Ok(0) => break,
            Ok(_) => {
                let line = line_buf.trim_end_matches(&['\n', '\r'][..]);
                engine.colorize_line(line, writer);
                if line_flush {
                    let _ = writer.flush();
                }
            }
            Err(e) => {
                // PTY returns EIO when child exits — that's normal
                if e.raw_os_error() == Some(EIO) {
                    break;
                }
                break;
            }
        }
    }
}

// --- libc FFI for PTY ---

#[repr(C)]
#[allow(non_camel_case_types)]
struct libc_termios {
    c_iflag: u32,
    c_oflag: u32,
    c_cflag: u32,
    c_lflag: u32,
    c_line: u8,
    c_cc: [u8; 32],
    c_ispeed: u32,
    c_ospeed: u32,
}

// c_iflag
const IGNBRK: u32 = 0o1;
const BRKINT: u32 = 0o2;
const PARMRK: u32 = 0o10;
const ISTRIP: u32 = 0o40;
const INLCR: u32 = 0o100;
const IGNCR: u32 = 0o200;
const ICRNL: u32 = 0o400;
const IXON: u32 = 0o2000;
const IXOFF: u32 = 0o10000;
// c_oflag
const OPOST: u32 = 0o1;
const ONLCR: u32 = 0o4;
// c_lflag
const ISIG: u32 = 0o1;
const ICANON: u32 = 0o2;
const ECHO: u32 = 0o10;
const ECHONL: u32 = 0o100;
const IEXTEN: u32 = 0o100000;
// c_cflag
const CSIZE: u32 = 0o60;
const PARENB: u32 = 0o400;
const CS8: u32 = 0o60;
// ioctl
const TIOCSWINSZ: u64 = 0x5414;

#[repr(C)]
struct Winsize {
    ws_row: u16,
    ws_col: u16,
    ws_xpixel: u16,
    ws_ypixel: u16,
}

unsafe extern "C" {
    fn openpty(
        master: *mut RawFd,
        slave: *mut RawFd,
        name: *mut u8,
        termp: *const libc_termios,
        winp: *const (),
    ) -> i32;
    fn tcgetattr(fd: RawFd, termios: *mut libc_termios) -> i32;
    fn tcsetattr(fd: RawFd, action: i32, termios: *const libc_termios) -> i32;
    fn close(fd: RawFd) -> i32;
    fn ioctl(fd: RawFd, request: u64, ...) -> i32;
}
