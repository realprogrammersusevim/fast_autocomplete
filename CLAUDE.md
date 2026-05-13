# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`fast_autocomplete` is a Rust daemon that provides low-latency shell tab-completion over a Unix domain socket. A long-running daemon harvests zsh's completion tree once at startup, then answers completion requests from zsh (or any client) without spawning subprocesses per-keystroke.

## Build & run

```sh
cargo build --release          # production binary → target/release/fast_autocomplete
cargo build                    # debug build
cargo run                      # start the daemon (debug)
cargo test                     # all tests (unit tests live in every src/*.rs module)
cargo test parser              # run only tests in src/parser.rs (module name filter)
zsh tests/test_harvester.sh    # snapshot tests for scripts/harvester.zsh
```

Harvester snapshots live in `tests/harvester_snapshots/`. Run `zsh tests/test_harvester.sh --update` to regenerate them after an intentional change.

Socket path is `$TMPDIR/fast_autocomplete_<uid>.sock` by default; override with `$FAST_AUTOCOMPLETE_SOCKET`. Enable logging with `RUST_LOG=debug cargo run`.

To rebuild and hot-swap a running daemon: `scripts/update.sh` (builds release, kills old daemon, starts new one).

## Manual socket testing

Send a raw request to the running daemon with `socat` or `nc`:

```sh
# socat (preferred)
printf 'BUFFER=git \nCURSOR=5\nCWD=%s\nSESSION=test\n\n' "$PWD" \
  | socat - UNIX-CONNECT:"$TMPDIR/fast_autocomplete_$(id -u).sock"

# nc fallback
printf 'BUFFER=git \nCURSOR=5\nCWD=%s\nSESSION=test\n\n' "$PWD" \
  | nc -U "$TMPDIR/fast_autocomplete_$(id -u).sock"
```

The response is a single JSON line: `{"completions":[...],"unchanged":false}`. Pipe through `jq` to pretty-print. Change `BUFFER`/`CURSOR` to test different inputs; `CURSOR` must equal the byte offset of the cursor in `BUFFER`.

## Architecture

```
zsh plugin
    │  (invokes binary subcommands for socket I/O)
    ▼
main.rs (subcommand dispatch)
    ├─ (no args)       → daemon_main() — async tokio daemon
    ├─ --complete      → client::run_complete()  — query daemon, print completions to stdout
    ├─ --display <cols>→ client::run_display()   — query daemon, format columns for inline display
    └─ --record <val>  → client::run_record()    — send RECORD= request to update frecency

client.rs          — Rust socket client (replaces zsh socket I/O); handles connect/send/recv,
                     column formatting for inline display, and fire-and-forget frecency recording

socket.rs          — accept loop; per-connection handler; JIT harvest trigger
    │
    ├─ parser.rs   — splits BUFFER at CURSOR into (lookup_words, current_word)
    ├─ cache.rs    — CompletionTree: trie of CompletionNode {flags, subcommands, wants_files, children}
    ├─ files.rs    — filesystem listing with ~ and relative-path expansion
    ├─ frecency.rs — in-memory frequency×recency scorer (in-process only, not persisted)
    ├─ ranking.rs  — merge static + file items → fuzzy-filter (skim scorer) → dedup → sort by frecency+fuzzy/type/alpha → cap 200; flags only shown when typing `-`
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
- `cmd_names: OnceLock<Vec<String>>` — all known command names (zsh completion table + `ZSH_BUILTINS` constant), populated quickly at startup
- `harvest_channels: DashMap<String, Arc<watch::Sender<bool>>>` — one channel per command; signals completion
- `harvested: DashMap<String, ()>` — set of commands whose harvest has finished
- `sessions: DashMap<u64, SessionState>`, `frecency: Mutex<FrecencyStore>`

**JIT harvest flow** (`socket.rs::ensure_harvested`): on the first request for a command, a `spawn_blocking` task runs `harvest_command()` and inserts results into the tree; `harvested` is marked when done. The function returns immediately — requests don't wait for the harvest; they get file-only completions until the tree is populated. Concurrent requests for the same command are deduped via `harvest_channels`.

**Request protocol** (`protocol.rs`): newline-delimited `KEY=VALUE` block terminated by a blank line. Fields: `BUFFER`, `CURSOR`, `CWD`, `SESSION`. Optional `RECORD=<value>` skips completion and records a frecency hit instead — daemon returns `{"unchanged":true}`. Response: single JSON line `{"completions":[...],"unchanged":false}` or `{"unchanged":true}`.

**Harvester script** (`scripts/harvester.zsh`): run inside a zsh subshell with the target command as `$1`; intercepts `compadd`/`_arguments`/`_describe` to extract flags and subcommands without executing external processes. Emits NDJSON, one object per completion node. The Rust side embeds this script via `include_str!` and writes it to a temp file before executing. The `list_commands()` path uses a separate inline script (`LIST_COMMANDS_SCRIPT` in `harvester.rs`) that is never written to `scripts/`.

**zsh plugin** (`fast_autocomplete.plugin.zsh`): source this file (or let a plugin manager load it). It registers `_fast_autocomplete` as the first completer, auto-launches the daemon if the socket is absent, and falls through to `_complete` + `_files` on failure. All socket I/O is delegated to the binary via subcommands (`fast_autocomplete --complete`, `--display <cols>`, `--record <val>`) — the plugin no longer does raw socket I/O itself. Before querying the daemon, `_fa_alias_expand` resolves simple (no metacharacter) aliases so that aliased commands get proper completions. Also installs `zle-line-pre-redraw` / `zle-line-finish` hooks (`_fa_update_below` / `_fa_clear_below`) that render completions in a columnar display below the prompt via `zle -M` as you type — separate from Tab completion. When the user accepts a completion, the plugin calls `fast_autocomplete --record <value>` to update the frecency store.

**Key design constraints:**
- Harvests are lazy and per-command — the daemon is immediately usable (file-only fallback) before any harvest runs.
- Each connection handles exactly one request then closes.
- Frecency is in-memory only; it resets when the daemon restarts.
- The harvester recurses up to depth 3 (command → subcommand → sub-subcommand) to avoid combinatorial explosion.
- Single-instance enforcement via `flock` on a `.lock` file (same base path as the socket with `.lock` extension). A non-blocking `LOCK_EX` attempt at startup exits immediately if another daemon holds the lock — eliminates the TOCTOU race of socket-based detection.
- Any leftover socket from a crashed daemon is removed at startup before binding.
- `harvest_command()` wraps the child process in a `KillOnDrop` guard (defined inline) so the child is always killed and reaped on every exit path, preventing zombies.
- Flags (items starting with `-`) are only shown when `current_word` itself starts with `-` — keeps completions uncluttered while typing subcommands or paths.
- Completions are fuzzy-filtered using the skim algorithm (`fuzzy_matcher` crate): frecency dominates scoring; the fuzzy score breaks ties among zero-frecency items. Items with no fuzzy match are dropped entirely.
- Alias expansion: the zsh plugin resolves simple aliases (no shell metacharacters) before querying the daemon, so `g status` expands to `git status` and gets proper completions.
