# fast_autocomplete

Fast tab-completion for zsh. A Rust daemon harvests completions once per command
and serves them instantly over a Unix socket.

As you type, completions appear in a columnar display below the prompt. Results
are ranked by frecency and fuzzy-matched.

This project was built with two main goals in mind:

1. Be very fast
2. Not leak zsh processes like `zsh-autocomplete` tended to.

## Installation

1. Build the binary and put it on your `$PATH`:

   ```sh
   cargo build --release
   cp target/release/fast_autocomplete ~/.local/bin/ # or literally any directory in your path
   ```

2. Source the plugin in your `.zshrc`:
   ```zsh
   source /path/to/fast_autocomplete.plugin.zsh # or your package manager's equivalent
   ```

The daemon starts automatically when you open a shell.

## Usage

Tab-completion works as normal. On first use of a command, completions are
harvested in the background, file completions are shown in the meantime.
Subsequent requests are instant.

Accepting a completion updates the frecency store so your most-used completions
rank higher over time.

## Configuration

| Variable                           | Default                                                   | Purpose        |
| ---------------------------------- | --------------------------------------------------------- | -------------- |
| `$FAST_AUTOCOMPLETE_SOCKET`        | `~/.cache/fast_autocomplete/fast_autocomplete_<uid>.sock` | Socket path    |
| `$FAST_AUTOCOMPLETE_FRECENCY_PATH` | `~/.local/share/fast_autocomplete/frecency.bin`           | Frecency store |

## Updating

```sh
scripts/update.sh
```

Builds a new release binary and hot-swaps the running daemon.

## Note

This project was almost completely vibe coded and built for myself. It's small
enough that nearly any coding agent can easily understand it and tweak it if you
want to change something.
