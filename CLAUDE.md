# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`fast_autocomplete` is a Rust daemon that provides low-latency shell tab-completion over a Unix domain socket. A long-running daemon harvests zsh's completion tree once at startup, then answers completion requests from zsh (or any client) without spawning subprocesses per-keystroke.

## Build & run

```sh
cargo build --release          # production binary → target/release/fast_autocomplete
cargo build                    # debug build
cargo run                      # start the daemon (debug)
cargo test                     # all tests
cargo test parser              # run only tests in src/parser.rs (module name filter)
```

Socket path is `$TMPDIR/fast_autocomplete_<uid>.sock` by default; override with `$FAST_AUTOCOMPLETE_SOCKET`. Enable logging with `RUST_LOG=debug cargo run`.

## Architecture

```
zsh client
    │  (Unix socket, one request/connection)
    ▼
socket.rs          — accept loop; per-connection handler; JIT harvest trigger
    │
    ├─ parser.rs   — splits BUFFER at CURSOR into (lookup_words, current_word)
    ├─ cache.rs    — CompletionTree: trie of CompletionNode {flags, subcommands, wants_files, children}
    ├─ files.rs    — filesystem listing with ~ and relative-path expansion
    ├─ frecency.rs — in-memory frequency×recency scorer (in-process only, not persisted)
    ├─ ranking.rs  — merge static + file items → prefix-filter → dedup → sort by frecency/type/alpha → cap 200
    └─ session.rs  — per-session hash dedup (sends `unchanged:true` if list hasn't changed)

harvester.rs       — two entry points:
                       list_commands()     — fast startup: enumerates commands on $PATH/fpath without
                                             running any completion functions; result stored in
                                             SharedState.cmd_names (OnceLock<Vec<String>>)
                       harvest_command()   — JIT per-command: spawns scripts/harvester.zsh for one
                                             command via spawn_blocking; parses NDJSON into CompletionTree
```

**SharedState** (defined in `main.rs`):
- `tree: Arc<CompletionTree>` — shared trie, populated incrementally as commands are harvested
- `cmd_names: OnceLock<Vec<String>>` — all known command names, populated quickly at startup
- `harvest_channels: DashMap<String, Arc<watch::Sender<bool>>>` — one channel per command; signals completion
- `harvested: DashMap<String, ()>` — set of commands whose harvest has finished
- `sessions: DashMap<u64, SessionState>`, `frecency: Mutex<FrecencyStore>`

**JIT harvest flow** (`socket.rs::ensure_harvested`): on the first request for a command, a `spawn_blocking` task runs `harvest_command()` and sends `true` on the watch channel when done. Concurrent requests for the same command subscribe to the same channel and wait up to 300 ms. Subsequent requests skip the check via `harvested`.

**Request protocol** (`protocol.rs`): newline-delimited `KEY=VALUE` block terminated by a blank line. Fields: `BUFFER`, `CURSOR`, `CWD`, `SESSION`. Response: single JSON line `{"completions":[...],"unchanged":false}` or `{"unchanged":true}`.

**Harvester script** (`scripts/harvester.zsh`): run inside a zsh subshell with the target command as `$1`; intercepts `compadd`/`_arguments`/`_describe` to extract flags and subcommands without executing external processes. Emits NDJSON, one object per completion node. The Rust side embeds this script via `include_str!` and writes it to a temp file before executing.

**zsh plugin** (`fast_autocomplete.plugin.zsh`): source this file (or let a plugin manager load it). It registers `_fast_autocomplete` as the first completer, auto-launches the daemon if the socket is absent, and falls through to `_complete` + `_files` on failure. Prefers `socat` for socket I/O, falls back to `nc -U`.

**Key design constraints:**
- Harvests are lazy and per-command — the daemon is immediately usable (file-only fallback) before any harvest runs.
- Each connection handles exactly one request then closes.
- Frecency is in-memory only; it resets when the daemon restarts.
- The harvester recurses up to depth 3 (command → subcommand → sub-subcommand) to avoid combinatorial explosion.
- Stale-socket detection at startup: if the socket file exists and accepts a connection, a second daemon instance exits immediately.
