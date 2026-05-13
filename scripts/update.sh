#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
SOCKET="${FAST_AUTOCOMPLETE_SOCKET:-${TMPDIR:-/tmp}/fast_autocomplete_$(id -u).sock}"

echo "Building release binary..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

echo "Stopping running daemon..."
if pid=$(lsof "$SOCKET" -t 2>/dev/null | head -1); then
    kill "$pid" && echo "Stopped PID $pid"
else
    echo "No running daemon found"
fi

# Remove the socket file so the new daemon can bind without seeing the old one
# as a live peer (otherwise stale-socket detection would cause it to exit).
rm -f "$SOCKET"

echo "Starting new daemon..."
"$REPO_DIR/target/release/fast_autocomplete" &
new_pid=$!
disown

echo "Done (PID $new_pid)"
