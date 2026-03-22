# rgc

Blazing fast command output colorizer written in Rust. Drop-in alternative to [grc](https://github.com/garabik/grc) with bundled YAML configs for 100+ commands, automatic pipe detection, and zero runtime dependencies — everything in a single ~2.5 MB binary.

## Disclaimer

This project was entirely written with the help of AI as a proof of concept. It is provided "as is", without warranty of any kind, express or implied. The author does not bear any responsibility and does not guarantee correctness, reliability, fitness for any particular purpose, or continued maintenance. Use at your own risk.

## Quick Start

```bash
# colorize any supported command
rgc ping 8.8.8.8
rgc docker ps
rgc df -h

# pipe mode — auto-detects the command
mount | rgc
ps aux | rgc

# generate shell aliases so every command is colorized automatically
source <(rgc --aliases)
```

## Installation

Download from [releases](../../releases) or build from source:

```bash
cargo build --release
sudo cp target/release/rgc /usr/local/bin/
```

## How It Works

```
rgc mount          # spawns mount in a PTY, colorizes stdout
mount | rgc        # detects "mount" via process group, colorizes stdin
rgc -c ping < file # uses explicit config
```

1. **Config resolution** — command name is matched against `command:` fields in YAML configs via O(1) HashMap lookup. Compound commands like `docker ps` and `ip addr` are matched with subcommand awareness
2. **Pipe auto-detection** — in pipe mode, rgc finds the writing process by scanning `/proc` for process group siblings. A pre-main `.init_array` hook captures the peer before Rust's runtime starts, catching even fast-exiting commands
3. **PTY streaming** — in direct mode, the child runs in a pseudo-terminal so it line-buffers its output. Real-time streaming for `tcpdump`, `ping`, `tail -f`, etc.
4. **Dual regex engine** — patterns are compiled with the `regex` crate (SIMD-accelerated, linear-time) first. Only patterns with lookbehind/lookahead fall back to `fancy-regex`
5. **Wrapper stripping** — `rgc sudo tcpdump` and `sudo tcpdump | rgc` both correctly resolve to the tcpdump config

## Benchmarks

Measured with [hyperfine](https://github.com/sharkdp/hyperfine). Linux 6.6 (WSL2), Intel i5-13600KF, Rust 1.94, grc 1.13. Mean of 10 runs, 2 warmup runs. "bare" = command without any colorizer.

#### Command execution (end-to-end wall-clock)

| Command | bare | rgc | grc | rgc overhead | grc overhead | Speed difference |
|---|---|---|---|---|---|---|
| `env` | 0.4 ms | 1.4 ms | 34.5 ms | +1.0 ms | +34.1 ms | **24x** |
| `mount` | 0.7 ms | 2.1 ms | 33.2 ms | +1.4 ms | +32.5 ms | **16x** |
| `lsmod` | 0.9 ms | 1.9 ms | 34.2 ms | +1.0 ms | +33.3 ms | **18x** |
| `ip addr` | 1.1 ms | 3.3 ms | 34.0 ms | +2.2 ms | +32.9 ms | **10x** |
| `lsblk` | 1.5 ms | 4.1 ms | 35.3 ms | +2.6 ms | +33.8 ms | **8x** |
| `df -h` | 1.4 ms | 4.7 ms | 34.0 ms | +3.3 ms | +32.6 ms | **7x** |
| `netstat -tln` | 2.3 ms | 6.0 ms | 32.9 ms | +3.7 ms | +30.6 ms | **5x** |
| `ps aux` | 2.3 ms | 10.4 ms | 35.4 ms | +8.1 ms | +33.1 ms | **3x** |
| `docker network ls` | 10.5 ms | 12.4 ms | 34.7 ms | +1.9 ms | +24.2 ms | **3x** |
| `docker ps` | 10.2 ms | 14.2 ms | 36.0 ms | +4.0 ms | +25.8 ms | **2.5x** |

#### Throughput (1M lines piped through colorizer)

| Config | rgc | grc | Speed difference |
|---|---|---|---|
| `log` (5 rules) | 0.55 s | 4.46 s | **8x** |
| `ps` (17 rules) | 2.45 s | 7.98 s | **3x** |

#### Streaming (I/O-bound, wall-clock includes network wait)

| Command | bare | rgc | grc |
|---|---|---|---|
| `ping -c 3 127.0.0.1` | 2.002 s | 2.025 s | 2.043 s |

## rgc vs grc

| | rgc | grc |
|---|---|---|
| **Binary** | Single ~2.5 MB binary, all configs bundled | Python runtime + config files in `/usr/share/grc/` |
| **Startup** | ~2 ms | ~33 ms |
| **Config format** | YAML with palette, named groups, `when` filtering | Custom INI-like with `======` separators |
| **Regex** | SIMD-accelerated `regex` + `fancy-regex` fallback | Python `re` |
| **Colors** | Standard 16, 256-color, RGB | Standard 16 only |
| **Streaming** | PTY-based, always line-buffered | `--pty` flag (experimental) |
| **Pipe detection** | Auto-detects via process group | Requires separate `grcat` binary + explicit config |
| **Config source** | `command:` field in each YAML config | Separate `grc.conf` mapping file |
| **External configs** | `-d <path>` with auto-discovery | `~/.grc/` directory |

## Supported Commands

Run `rgc --list` to see all supported commands. Config files are in the [`configs/`](configs/) directory — each YAML file defines which commands it handles and the colorization rules for them.

Syntax highlighters without a bound command (use with `-c`): `common`, `log`, `sql`, `yaml`, `php`, `jobs`.

## Config Format

Configs use YAML with the following structure:

```yaml
command:
  - ping
  - ping6

palette:
  ip: bright_blue

rules:
  - name: IP address
    pattern: '\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}'
    colors: [ip]

  - name: icmp_seq
    pattern: 'icmp_seq=(\d+)'
    colors: [default, yellow]
    count: more
```

### Fields

| Field | Description |
|---|---|
| `command` | List of commands this config applies to. Supports subcommands (`docker ps`, `ip addr`) |
| `palette` | Reusable color aliases resolved at parse time |
| `rules[].pattern` | Regex pattern. Supports lookahead/lookbehind via `fancy-regex` fallback |
| `rules[].colors` | Positional list or named map (for `(?P<name>...)` groups) |
| `rules[].name` | Optional rule description |
| `rules[].when` | Scope rule to specific commands in merged configs |
| `rules[].count` | `more` (default), `once`, `stop`, `previous`, `block`, `unblock` |
| `rules[].skip` | Skip the line entirely if matched |
| `rules[].replace` | Replacement string (supports `$1`, `$2` backreferences) |

### Colors

Standard: `red`, `green`, `yellow`, `blue`, `magenta`, `cyan`, `white`, `black` and `bright_*` variants. Prefixed with `on_` for background. Modifiers: `bold`, `underline`, `italic`, `dim`, `blink`, `reverse`, `hidden`. Special: `default` (no change for group 0), `unchanged`, `previous`.

Extended: `color256(N)`, `on_color256(N)`, `rgb(R,G,B)`, `on_rgb(R,G,B)`.

### Named Capture Groups

```yaml
- pattern: '(?P<key>\w+)=(?P<val>.+)'
  colors:
    key: bold cyan
    val: green
```

### Merged Configs with `when`

```yaml
command:
  - docker ps
  - docker images

rules:
  - pattern: HEADERS
    colors: [underline]
    when: [docker ps]
```

Use `-d <path>` to load configs from a directory instead of bundled ones.

## Environment Variables

| Variable | Effect |
|---|---|
| `NO_COLOR` | Disable all colorization (output passes through unmodified) |
| `FORCE_COLOR` | Force colorization even when stdout is not a TTY |

## Limitations

- Linux only (uses `/proc`, PTY, `.init_array`)
- Pipe auto-detection relies on process group scanning — ultra-fast commands like `env` may exit before detection. Use direct mode (`rgc env`) or `-c` for these
- In pipe mode, upstream buffering is controlled by the sender, not rgc. Use direct mode for real-time streaming

## License

Config files are based on [grc](https://github.com/garabik/grc) by Radovan Garabik (GPL-2.0). The rest of the code is GPL-2.0 too.
