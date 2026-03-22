// SPDX-License-Identifier: GPL-2.0-or-later
// Copyright (C) 2026 flameshikari

//! Pre-main pipe peer detection using raw syscalls.
//! Runs via `.init_array` before Rust's runtime initializes,
//! catching even ultra-fast piped commands like `env` or `id`.

#![allow(clippy::declare_interior_mutable_const, clippy::manual_c_str_literals)]

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

/// Buffer to store the detected peer command (null-terminated).
static PEER_CMD_BUF: [AtomicU8; 256] = {
    const INIT: AtomicU8 = AtomicU8::new(0);
    [INIT; 256]
};
static PEER_CMD_VALID: AtomicBool = AtomicBool::new(false);

/// Read the pre-main detected peer command, if any.
pub fn get_early_peer_cmd() -> Option<String> {
    if !PEER_CMD_VALID.load(Ordering::Relaxed) {
        return None;
    }
    let mut buf = [0u8; 256];
    for (i, a) in PEER_CMD_BUF.iter().enumerate() {
        buf[i] = a.load(Ordering::Relaxed);
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    if len == 0 {
        return None;
    }
    Some(String::from_utf8_lossy(&buf[..len]).into_owned())
}

fn store_peer_cmd(byte: u8, idx: usize) {
    if idx < 256 {
        PEER_CMD_BUF[idx].store(byte, Ordering::Relaxed);
    }
}

// --- Architecture-independent helpers (Linux only) ---

#[cfg(target_os = "linux")]
mod common {
    /// linux_dirent64 struct layout (architecture-independent on Linux).
    #[repr(C)]
    pub struct Dirent64 {
        pub d_ino: u64,
        pub d_off: i64,
        pub d_reclen: u16,
        pub d_type: u8,
        // d_name follows (variable length, null-terminated)
    }

    /// Format a u32 into a decimal string in a buffer. Returns slice length.
    pub fn fmt_u32(mut n: u32, buf: &mut [u8]) -> usize {
        if n == 0 {
            buf[0] = b'0';
            return 1;
        }
        let mut tmp = [0u8; 10];
        let mut len = 0;
        while n > 0 {
            tmp[len] = b'0' + (n % 10) as u8;
            n /= 10;
            len += 1;
        }
        for i in 0..len {
            buf[i] = tmp[len - 1 - i];
        }
        len
    }

    /// Parse decimal digits from a byte slice, returns (value, ok).
    pub fn parse_u32(s: &[u8]) -> (u32, bool) {
        let mut val: u32 = 0;
        if s.is_empty() {
            return (0, false);
        }
        for &b in s {
            if !b.is_ascii_digit() {
                return (0, false);
            }
            val = val * 10 + (b - b'0') as u32;
        }
        (val, true)
    }
}

// --- Raw syscall wrappers (no std, no libc crate) ---

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod raw {
    use core::arch::asm;

    const SYS_READ: usize = 0;
    const SYS_OPEN: usize = 2;
    const SYS_CLOSE: usize = 3;
    const SYS_GETPID: usize = 39;
    const SYS_GETDENTS64: usize = 217;
    pub const O_RDONLY: usize = 0;

    #[inline(always)]
    unsafe fn syscall0(nr: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "syscall",
                in("rax") nr,
                lateout("rax") ret,
                out("rcx") _,
                out("r11") _,
                options(nostack),
            );
        }
        ret
    }

    #[inline(always)]
    unsafe fn syscall3(nr: usize, a1: usize, a2: usize, a3: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "syscall",
                in("rax") nr,
                in("rdi") a1,
                in("rsi") a2,
                in("rdx") a3,
                lateout("rax") ret,
                out("rcx") _,
                out("r11") _,
                options(nostack),
            );
        }
        ret
    }

    pub unsafe fn getpid() -> i32 {
        unsafe { syscall0(SYS_GETPID) as i32 }
    }

    pub unsafe fn open(path: *const u8, flags: usize) -> i32 {
        unsafe { syscall3(SYS_OPEN, path as usize, flags, 0) as i32 }
    }

    pub unsafe fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
        unsafe { syscall3(SYS_READ, fd as usize, buf as usize, count) }
    }

    pub unsafe fn close(fd: i32) -> i32 {
        let ret: isize;
        unsafe {
            asm!(
                "syscall",
                in("rax") SYS_CLOSE,
                in("rdi") fd as usize,
                lateout("rax") ret,
                out("rcx") _,
                out("r11") _,
                options(nostack),
            );
        }
        ret as i32
    }

    pub unsafe fn getdents64(fd: i32, buf: *mut u8, count: usize) -> isize {
        unsafe { syscall3(SYS_GETDENTS64, fd as usize, buf as usize, count) }
    }

    /// Read a file into a buffer, returns bytes read.
    pub unsafe fn read_file(path: &[u8], buf: &mut [u8]) -> isize {
        // path must be null-terminated
        let fd = unsafe { open(path.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return -1;
        }
        let n = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
        unsafe { close(fd) };
        n
    }
}

#[cfg(all(target_os = "linux", target_arch = "aarch64"))]
mod raw {
    use core::arch::asm;

    // aarch64 Linux syscall numbers (different from x86_64)
    const SYS_READ: usize = 63;
    const SYS_OPENAT: usize = 56; // aarch64 has no SYS_OPEN, uses openat
    const SYS_CLOSE: usize = 57;
    const SYS_GETPID: usize = 172;
    const SYS_GETDENTS64: usize = 61;
    pub const O_RDONLY: usize = 0;
    const AT_FDCWD: isize = -100;

    #[inline(always)]
    unsafe fn syscall1(nr: usize, a1: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "svc #0",
                in("x8") nr,
                inlateout("x0") a1 as isize => ret,
                options(nostack),
            );
        }
        ret
    }

    #[inline(always)]
    unsafe fn syscall3(nr: usize, a1: usize, a2: usize, a3: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "svc #0",
                in("x8") nr,
                inlateout("x0") a1 as isize => ret,
                in("x1") a2,
                in("x2") a3,
                options(nostack),
            );
        }
        ret
    }

    #[inline(always)]
    unsafe fn syscall4(nr: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> isize {
        let ret: isize;
        unsafe {
            asm!(
                "svc #0",
                in("x8") nr,
                inlateout("x0") a1 as isize => ret,
                in("x1") a2,
                in("x2") a3,
                in("x3") a4,
                options(nostack),
            );
        }
        ret
    }

    pub unsafe fn getpid() -> i32 {
        unsafe { syscall1(SYS_GETPID, 0) as i32 }
    }

    pub unsafe fn open(path: *const u8, flags: usize) -> i32 {
        // aarch64 uses openat(AT_FDCWD, path, flags, 0)
        unsafe { syscall4(SYS_OPENAT, AT_FDCWD as usize, path as usize, flags, 0) as i32 }
    }

    pub unsafe fn read(fd: i32, buf: *mut u8, count: usize) -> isize {
        unsafe { syscall3(SYS_READ, fd as usize, buf as usize, count) }
    }

    pub unsafe fn close(fd: i32) -> i32 {
        unsafe { syscall1(SYS_CLOSE, fd as usize) as i32 }
    }

    pub unsafe fn getdents64(fd: i32, buf: *mut u8, count: usize) -> isize {
        unsafe { syscall3(SYS_GETDENTS64, fd as usize, buf as usize, count) }
    }

    pub unsafe fn read_file(path: &[u8], buf: &mut [u8]) -> isize {
        let fd = unsafe { open(path.as_ptr(), O_RDONLY) };
        if fd < 0 {
            return -1;
        }
        let n = unsafe { read(fd, buf.as_mut_ptr(), buf.len()) };
        unsafe { close(fd) };
        n
    }
}

#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
unsafe fn early_detect_peer() {
    use common::*;
    use raw::*;

    let my_pid = unsafe { getpid() } as u32;

    // Read our PGID from /proc/self/stat
    let mut stat_buf = [0u8; 512];
    let n = unsafe { read_file(b"/proc/self/stat\0", &mut stat_buf) };
    if n <= 0 {
        return;
    }
    let stat_str = &stat_buf[..n as usize];

    // Parse PGID: format is "pid (comm) state ppid pgid ..."
    let Some(comm_end) = stat_str.iter().rposition(|&b| b == b')') else {
        return;
    };
    let after_comm = &stat_str[comm_end + 1..];
    // After ')': " S ppid pgid ..."
    // fields: [0]=state, [1]=ppid, [2]=pgid
    let mut field_starts = [0usize; 4];
    let mut field_ends = [0usize; 4];
    let mut fc = 0;
    let mut in_f = false;
    for (i, &b) in after_comm.iter().enumerate() {
        if b == b' ' || b == b'\t' || b == b'\n' {
            if in_f {
                field_ends[fc] = i;
                fc += 1;
                in_f = false;
                if fc >= 4 {
                    break;
                }
            }
        } else if !in_f {
            field_starts[fc] = i;
            in_f = true;
        }
    }
    if fc < 3 {
        return;
    }

    let (my_ppid, ok1) = parse_u32(&after_comm[field_starts[1]..field_ends[1]]);
    let (my_pgid, ok2) = parse_u32(&after_comm[field_starts[2]..field_ends[2]]);
    if !ok1 || !ok2 {
        return;
    }

    // Scan /proc for processes with same PGID
    let proc_fd = unsafe { open(b"/proc\0".as_ptr(), 0x10000) }; // O_RDONLY | O_DIRECTORY
    if proc_fd < 0 {
        return;
    }

    let mut dents_buf = [0u8; 4096];
    loop {
        let n = unsafe { getdents64(proc_fd, dents_buf.as_mut_ptr(), dents_buf.len()) };
        if n <= 0 {
            break;
        }
        let mut offset = 0;
        while offset < n as usize {
            let dent = unsafe { &*(dents_buf.as_ptr().add(offset) as *const Dirent64) };
            let name_ptr = unsafe { dents_buf.as_ptr().add(offset + 19) }; // offset of d_name
            let name_len = {
                let mut l = 0;
                while unsafe { *name_ptr.add(l) } != 0 && l < 20 {
                    l += 1;
                }
                l
            };
            let name_slice = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
            let (pid, is_num) = parse_u32(name_slice);

            if is_num && pid != my_pid && pid != my_ppid {
                // Read this process's stat to check PGID
                let mut path_buf = [0u8; 64];
                let mut pos = 0;
                for &b in b"/proc/" {
                    path_buf[pos] = b;
                    pos += 1;
                }
                let pid_len = fmt_u32(pid, &mut path_buf[pos..]);
                pos += pid_len;
                for &b in b"/stat\0" {
                    path_buf[pos] = b;
                    pos += 1;
                }

                let mut peer_stat = [0u8; 512];
                let sn = unsafe { read_file(&path_buf[..pos], &mut peer_stat) };
                if sn > 0 {
                    let peer_str = &peer_stat[..sn as usize];
                    if let Some(ce) = peer_str.iter().rposition(|&b| b == b')') {
                        let pa = &peer_str[ce + 1..];
                        let mut pfc = 0;
                        let mut pfs = [0usize; 4];
                        let mut pfe = [0usize; 4];
                        let mut inf = false;
                        for (i, &b) in pa.iter().enumerate() {
                            if b == b' ' || b == b'\t' || b == b'\n' {
                                if inf {
                                    pfe[pfc] = i;
                                    pfc += 1;
                                    inf = false;
                                    if pfc >= 3 {
                                        break;
                                    }
                                }
                            } else if !inf {
                                pfs[pfc] = i;
                                inf = true;
                            }
                        }
                        if pfc >= 3 {
                            let (peer_pgid, ok) = parse_u32(&pa[pfs[2]..pfe[2]]);
                            if ok && peer_pgid == my_pgid {
                                // Found a peer! Read its cmdline
                                pos = 0;
                                for &b in b"/proc/" {
                                    path_buf[pos] = b;
                                    pos += 1;
                                }
                                let pid_l = fmt_u32(pid, &mut path_buf[pos..]);
                                pos += pid_l;
                                for &b in b"/cmdline\0" {
                                    path_buf[pos] = b;
                                    pos += 1;
                                }

                                let mut cmdline = [0u8; 256];
                                let cn =
                                    unsafe { read_file(&path_buf[..pos], &mut cmdline) };
                                if cn > 0 {
                                    // Replace NUL separators with spaces
                                    let clen = cn as usize;
                                    let mut out_len = 0;
                                    for i in 0..clen {
                                        if cmdline[i] == 0 {
                                            if i + 1 < clen && cmdline[i + 1] != 0 {
                                                store_peer_cmd(b' ', out_len);
                                                out_len += 1;
                                            }
                                        } else {
                                            if out_len < 255 {
                                                store_peer_cmd(cmdline[i], out_len);
                                                out_len += 1;
                                            }
                                        }
                                    }
                                    store_peer_cmd(0, out_len);
                                    if out_len > 0 {
                                        PEER_CMD_VALID
                                            .store(true, Ordering::Relaxed);
                                        unsafe { close(proc_fd) };
                                        return;
                                    }
                                }

                                // Fallback: read comm from stat (already parsed)
                                let comm_start =
                                    peer_str.iter().position(|&b| b == b'(');
                                let comm_end_pos =
                                    peer_str.iter().rposition(|&b| b == b')');
                                if let (Some(cs), Some(ce2)) =
                                    (comm_start, comm_end_pos)
                                {
                                    let comm = &peer_str[cs + 1..ce2];
                                    let clen = comm.len().min(255);
                                    for (ci, &byte) in comm[..clen].iter().enumerate() {
                                        store_peer_cmd(byte, ci);
                                    }
                                    store_peer_cmd(0, clen);
                                    PEER_CMD_VALID.store(true, Ordering::Relaxed);
                                    unsafe { close(proc_fd) };
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            offset += dent.d_reclen as usize;
        }
    }

    unsafe { close(proc_fd) };
}

/// Pre-main entry point — runs before Rust's runtime via .init_array.
#[cfg(all(target_os = "linux", any(target_arch = "x86_64", target_arch = "aarch64")))]
#[used]
#[unsafe(link_section = ".init_array")]
static EARLY_INIT: unsafe extern "C" fn() = {
    unsafe extern "C" fn init() {
        unsafe { early_detect_peer() };
    }
    init
};
