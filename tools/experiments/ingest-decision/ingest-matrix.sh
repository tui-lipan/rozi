#!/usr/bin/env bash
# One application-level ingest run: a session server, attached probe client(s), and PANES panes
# that each `cat` the same corpus once. Derived from tools/memory-matrix.sh. Linux only.
# Usage: EXP_DIR=… ingest-matrix.sh BIN LABEL PANES CONTENT CLIENTS OUT.jsonl
set -euo pipefail

BIN=$1 LABEL=$2 PANES=$3 CONTENT=$4 CLIENTS=$5 OUT=$6
ROWS=${ROWS:-64} COLS=${COLS:-253} HISTORY=${HISTORY:-5000}
source "$(dirname -- "${BASH_SOURCE[0]}")/owned.sh"
CORPUS=$EXP_DIR/corpus/$CONTENT.txt
[[ -r $CORPUS ]] || { echo "missing corpus $CORPUS" >&2; exit 1; }

if [[ -n $(experiment_survivors) ]]; then
  echo "refusing to start: experiment processes still running:" >&2
  experiment_survivors >&2
  exit 1
fi

ROOT=$(mktemp -d "$EXP_DIR/run.XXXXXX")
SERVER_PID= WRAPPERS=() SOCKETS=() SESSION= CLIENT_PIDS=()
cleanup() {
  set +e
  # Clients first, while their pty is still alive, so none is left to lose its terminal mid-exit.
  owned_reap "${CLIENT_PIDS[@]:-}"
  [[ -n $SESSION ]] && "$BIN" sessions kill "$SESSION" >/dev/null 2>&1
  owned_reap "${WRAPPERS[@]:-}" "$SERVER_PID"
  for p in "${WRAPPERS[@]:-}" "$SERVER_PID"; do [[ -n $p ]] && wait "$p" 2>/dev/null; done
  rm -rf -- "$ROOT"
  if [[ -n $(experiment_survivors) ]]; then
    echo "LEAK: experiment processes survived cleanup:" >&2
    experiment_survivors >&2
    exit 3
  fi
}
trap cleanup EXIT INT TERM

mkdir -p "$ROOT"/{home,config,state,cache,data,runtime,work}
chmod 700 "$ROOT" "$ROOT/runtime"
export HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/config" XDG_STATE_HOME="$ROOT/state"
export XDG_CACHE_HOME="$ROOT/cache" XDG_DATA_HOME="$ROOT/data" XDG_RUNTIME_DIR="$ROOT/runtime"
unset ROZI_PANE ROZI_SOCKET ROZI_EXTENSION ROZI_BIN ROZI
export ROZI_CONFIG="$ROOT/config/config.toml" TERM=xterm-256color LANG=C.UTF-8 LC_ALL=C.UTF-8 SHELL=/bin/sh

workload() { # pane index
  printf 'while [ ! -e %s ]; do sleep 0.02; done; cat %s; : > %s; while :; do sleep 3600; done' \
    "$ROOT/work/start" "$CORPUS" "$ROOT/work/done-$1"
}
workload 1 >"$ROOT/work/w1.sh"
cat >"$ROZI_CONFIG" <<EOF
shell = ["/bin/sh", "$ROOT/work/w1.sh"]
command_shell = ["/bin/sh", "-c"]
cwd = "$ROOT/work"
scrollback = $HISTORY

[shell_integration]
mode = "off"

[session]
autosave = false
resurrect = false

[confirm]
kill_session = false

[animations]
enabled = false

[updates]
check = false
EOF

SESSION="ingest-$$"
"$BIN" --session "$SESSION" --fresh-server >"$ROOT/server.log" 2>&1 &
SERVER_PID=$!
own "$SERVER_PID"
for _ in $(seq 300); do compgen -G "$XDG_RUNTIME_DIR/rozi/session-*.sock" >/dev/null && break; sleep 0.05; done

for ((c = 0; c < CLIENTS; c++)); do
  script -qefc "stty rows $ROWS cols $COLS; exec '$BIN' sessions attach '$SESSION'" /dev/null </dev/null >"$ROOT/client-$c.log" 2>&1 &
  WRAPPERS+=("$!")
  own "$!"
  for _ in $(seq 300); do
    mapfile -t found < <(compgen -G "$XDG_RUNTIME_DIR/rozi/control-*.sock" || true)
    ((${#found[@]} > c)) && break
    sleep 0.05
  done
done
mapfile -t SOCKETS < <(compgen -G "$XDG_RUNTIME_DIR/rozi/control-*.sock")
((${#SOCKETS[@]} == CLIENTS)) || { echo "expected $CLIENTS clients, got ${#SOCKETS[@]}" >&2; cat "$ROOT"/*.log >&2; exit 1; }
for s in "${SOCKETS[@]}"; do
  p=${s##*/control-}; p=${p%.sock}
  own "$p"   # refuses anything that is not an experiment binary
  CLIENT_PIDS+=("$p")
done
control=${SOCKETS[0]}
for _ in $(seq 300); do "$BIN" --socket "$control" list-panes >/dev/null 2>&1 && break; sleep 0.05; done

for ((pane = 2; pane <= PANES; pane++)); do
  "$BIN" --socket "$control" split "$(workload "$pane")" >/dev/null
done
sleep 2

ticks() { # sum utime+stime of the given pids
  local total=0 p f
  for p in "$@"; do
    read -r -a f < <(sed 's/.*) //' "/proc/$p/stat")
    total=$((total + f[11] + f[12]))
  done
  echo "$total"
}

s0=$(ticks "$SERVER_PID"); c0=$(ticks "${CLIENT_PIDS[@]}")
t0=$(date +%s.%N)
: >"$ROOT/work/start"
for _ in $(seq 6000); do
  n=$( (compgen -G "$ROOT/work/done-*" || true) | wc -l)
  ((n >= PANES)) && break
  sleep 0.01
done
t1=$(date +%s.%N)
# Wait for server and client CPU to go quiet: no more than 1 tick in 300 ms.
prev=$(( $(ticks "$SERVER_PID") + $(ticks "${CLIENT_PIDS[@]}") ))
for _ in $(seq 60); do
  sleep 0.3
  now=$(( $(ticks "$SERVER_PID") + $(ticks "${CLIENT_PIDS[@]}") ))
  ((now - prev <= 1)) && break
  prev=$now
done
t2=$(date +%s.%N)
s1=$(ticks "$SERVER_PID"); c1=$(ticks "${CLIENT_PIDS[@]}")
sleep 1

pss() { local total=0 p v; for p in "$@"; do v=$(awk '/^Pss:/{print $2}' "/proc/$p/smaps_rollup"); total=$((total + v)); done; echo "$total"; }
rss() { local total=0 p v; for p in "$@"; do v=$(awk '/^VmRSS:/{print $2}' "/proc/$p/status"); total=$((total + v)); done; echo "$total"; }
server_pss=$(pss "$SERVER_PID"); client_pss=$(pss "${CLIENT_PIDS[@]}")
server_rss=$(rss "$SERVER_PID"); client_rss=$(rss "${CLIENT_PIDS[@]}")
lines=$(wc -l <"$CORPUS"); bytes=$(stat -c %s "$CORPUS")
python3 - "$OUT" <<PY
import json, sys
hz = 100
rec = dict(label="$LABEL", panes=$PANES, content="$CONTENT", clients=$CLIENTS, viewport="${COLS}x${ROWS}",
  history=$HISTORY, lines_per_pane=$lines, bytes_per_pane=$bytes,
  producer_wall_s=round($t1 - $t0, 4), quiet_wall_s=round($t2 - $t0, 4),
  server_cpu_s=($s1 - $s0) / hz, client_cpu_s=($c1 - $c0) / hz,
  server_pss_kib=$server_pss, client_pss_kib=$client_pss,
  server_rss_kib=$server_rss, client_rss_kib=$client_rss)
open(sys.argv[1], "a").write(json.dumps(rec) + "\n")
print(json.dumps(rec))
PY
