#!/bin/zsh
# Snapshot tests for scripts/harvester.zsh.
#
# Usage:
#   tests/test_harvester.sh            # run all snapshot comparisons
#   tests/test_harvester.sh --update   # regenerate snapshots from current output

setopt no_unset pipefail

REPO_DIR=${0:A:h:h}
HARVESTER="$REPO_DIR/scripts/harvester.zsh"
SNAP_DIR="$REPO_DIR/tests/harvester_snapshots"

# Commands whose output is snapshotted.
# Criteria: available on any macOS/Linux dev machine, stable completion functions.
SNAPSHOT_COMMANDS=(git grep man ls brew cd __nonexistent_cmd__)

UPDATE=0
[[ ${1:-} == --update ]] && UPDATE=1

if (( UPDATE )); then
  print -- "Updating snapshots..."
  for cmd in "${SNAPSHOT_COMMANDS[@]}"; do
    zsh "$HARVESTER" "$cmd" 2>/dev/null > "$SNAP_DIR/$cmd.ndjson"
    print -- "  updated $cmd.ndjson ($(wc -l < "$SNAP_DIR/$cmd.ndjson") lines)"
  done
  print -- "Done."
  exit 0
fi

pass=0
fail=0

for cmd in "${SNAPSHOT_COMMANDS[@]}"; do
  snap="$SNAP_DIR/$cmd.ndjson"
  if [[ ! -f $snap ]]; then
    print -u2 -- "MISSING  $cmd  (no snapshot at $snap; run --update to create)"
    (( fail++ ))
    continue
  fi

  actual=$(zsh "$HARVESTER" "$cmd" 2>/dev/null)
  expected=$(< "$snap")

  if [[ $actual == $expected ]]; then
    print -- "PASS     $cmd"
    (( pass++ ))
  else
    print -u2 -- "FAIL     $cmd"
    diff <(print -- "$expected") <(print -- "$actual") | head -40
    (( fail++ ))
  fi
done

print -- ""
print -- "Results: $pass passed, $fail failed"
(( fail == 0 ))
