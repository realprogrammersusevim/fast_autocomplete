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
socket.rs          — accept loop, per-connection handler
    │
    ├─ parser.rs   — splits BUFFER at CURSOR into (lookup_words, current_word)
    ├─ cache.rs    — CompletionTree: trie of CompletionNode {flags, subcommands, wants_files, children}
    ├─ files.rs    — filesystem listing with ~ and relative-path expansion
    ├─ frecency.rs — in-memory frequency×recency scorer (in-process only, not persisted)
    ├─ ranking.rs  — merge static + file items → prefix-filter → dedup → sort by frecency/type/alpha → cap 200
    └─ session.rs  — per-session hash dedup (sends `unchanged:true` if list hasn't changed)

harvester.rs       — spawns scripts/harvester.zsh at startup via spawn_blocking;
                     parses its NDJSON into CompletionTree; stored in SharedState.tree (OnceLock)
```

**Request protocol** (`protocol.rs`): newline-delimited `KEY=VALUE` block terminated by a blank line. Fields: `BUFFER`, `CURSOR`, `CWD`, `SESSION`. Response: single JSON line `{"completions":[...],"unchanged":false}` or `{"unchanged":true}`.

**Harvester** (`scripts/harvester.zsh`): run inside a zsh subshell; intercepts `compadd`/`_arguments`/`_describe` to extract flags and subcommands without executing external processes. Emits NDJSON, one object per completion node. The Rust side embeds this script via `include_str!` and writes it to a temp file before executing.

**Key design constraints:**
- `SharedState.tree` is an `OnceLock` — the daemon responds to requests immediately (with file-only fallback) while the harvester is still running in the background.
- Frecency is in-memory only; it resets when the daemon restarts.
- Each connection handles exactly one request then closes.
- The harvester recurses up to depth 3 (command → subcommand → sub-subcommand) to avoid combinatorial explosion.
