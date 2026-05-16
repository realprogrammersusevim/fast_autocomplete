#!/usr/bin/env bash
set -euo pipefail

REPO_DIR="$(cd "$(dirname "$0")/.." && pwd)"
if [[ -n ${FAST_AUTOCOMPLETE_SOCKET:-} ]]; then
    SOCKET="$FAST_AUTOCOMPLETE_SOCKET"
elif [[ -n ${XDG_RUNTIME_DIR:-} ]]; then
    SOCKET="${XDG_RUNTIME_DIR%/}/fast_autocomplete_$(id -u).sock"
else
    SOCKET="$HOME/.cache/fast_autocomplete/fast_autocomplete_$(id -u).sock"
fi
LOCK="${SOCKET%.sock}.lock"

echo "Building release binary..."
cargo build --release --manifest-path "$REPO_DIR/Cargo.toml"

echo "Stopping running daemon..."
# Prefer the lock file for detection — lsof on macOS finds regular files reliably.
pid=$(lsof -t "$LOCK" 2>/dev/null | head -1 || true)
if [[ -n $pid ]]; then
    kill "$pid" && echo "Stopped PID $pid" || true
    # Wait up to 2s for the lock to be released before starting the replacement.
    for i in {1..20}; do
        lsof -t "$LOCK" &>/dev/null || break
        sleep 0.1
    done
else
    echo "No running daemon found"
fi

rm -f "$SOCKET"

echo "Starting new daemon..."
"$REPO_DIR/target/release/fast_autocomplete" &
new_pid=$!
disown

echo "Done (PID $new_pid)"
