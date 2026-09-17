#!/usr/bin/env bash
# The decision matrix, interleaved so machine drift lands on both binaries equally.
# Usage: EXP_DIR=… REPS=5 run-matrix.sh OUT.jsonl [CONFIG...]   (CONFIG is "PANES CONTENT CLIENTS")
set -uo pipefail
: "${EXP_DIR:?set EXP_DIR to the directory holding rozi-dense and rozi-compact}"
OUT=${1:?output jsonl}
shift
REPS=${REPS:-5}
HERE=$(dirname -- "${BASH_SOURCE[0]}")
configs=("$@")
((${#configs[@]})) || configs=("1 plain 1" "4 plain 1" "8 plain 1" "1 log 1" "4 log 1" "8 log 1" "8 plain 2")
for rep in $(seq "$REPS"); do
  for cfg in "${configs[@]}"; do
    read -r panes content clients <<<"$cfg"
    if ((rep % 2)); then order=(dense compact); else order=(compact dense); fi
    for b in "${order[@]}"; do
      bash "$HERE/ingest-matrix.sh" "$EXP_DIR/rozi-$b" "$b" "$panes" "$content" "$clients" "$OUT" ||
        echo "{\"label\":\"$b\",\"panes\":$panes,\"content\":\"$content\",\"clients\":$clients,\"failed\":true}" >>"$OUT"
    done
  done
done
