#!/usr/bin/env bash
set -euo pipefail

MODE=quick
CUSTOM_CASE=()
OUTPUT_DIR="target/memory-matrix/$(date -u +%Y%m%dT%H%M%SZ)"
SETTLE_SECONDS=2
SAMPLE_COUNT=5
SAMPLE_INTERVAL=0.2

usage() {
  cat <<'EOF'
Usage: tools/memory-matrix.sh [--quick|--full|--lifecycle|--smoke] [--case ROWS COLS PANES HISTORY CONTENT CLIENTS [STATE]] [--output DIR]

Linux-only, opt-in PSS benchmark. --quick is the default; --smoke runs a bounded
image lifecycle to validate dependencies, replay, cleanup, and shutdown.
CONTENT is plain, styled, images, or image-stress. The stress variant emits enough
decoded pixels to exercise the client image budget. STATE is steady (default),
closed, disconnected, reconnected, or killed. --lifecycle records the full cleanup matrix.
EOF
}

while (($#)); do
  case "$1" in
    --quick|--full|--lifecycle|--smoke) MODE=${1#--}; shift ;;
    --case)
      (($# >= 7)) || { echo "--case requires ROWS COLS PANES HISTORY CONTENT CLIENTS" >&2; exit 2; }
      MODE=case
      CUSTOM_CASE=("$2" "$3" "$4" "$5" "$6" "$7")
      if (($# >= 8)) && [[ $8 != --* ]]; then
        CUSTOM_CASE+=("$8")
        shift 8
      else
        shift 7
      fi
      ;;
    --output) OUTPUT_DIR=${2:?--output requires a directory}; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

[[ $(uname -s) == Linux ]] || { echo "memory-matrix.sh requires Linux /proc" >&2; exit 1; }
for command in cargo python3 script stty; do
  command -v "$command" >/dev/null || { echo "missing required command: $command" >&2; exit 1; }
done

REPO=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$REPO"
mkdir -p "$OUTPUT_DIR"
OUTPUT_DIR=$(cd "$OUTPUT_DIR" && pwd)
JSONL="$OUTPUT_DIR/scenarios.jsonl"
: >"$JSONL"

echo "Building release binary..." >&2
cargo build --release
BIN="$REPO/target/release/rozi"

ROOT=
SERVER_PID=
WRAPPER_PIDS=()
CLIENT_PIDS=()
SESSION=
CONTROL_SOCKETS=()
ACTIVE_CLIENTS=0
PANE_IDS=()
PANE_MARKERS=()
PTY_DESCENDANT_PIDS=()
declare -A PTY_DESCENDANT_START=()

proc_start_time() {
  local pid=$1
  python3 - "$pid" <<'PY'
import pathlib, sys
try:
    stat = pathlib.Path(f"/proc/{sys.argv[1]}/stat").read_text()
    print(stat.rsplit(")", 1)[1].split()[19])
except (IndexError, OSError, ValueError):
    raise SystemExit(1)
PY
}

owned_pid_alive() {
  local pid=$1 expected current
  if [[ -n ${PTY_DESCENDANT_START[$pid]+captured} ]]; then
    expected=${PTY_DESCENDANT_START[$pid]}
    current=$(proc_start_time "$pid" 2>/dev/null) || return 1
    [[ $current == "$expected" ]]
  else
    kill -0 "$pid" >/dev/null 2>&1
  fi
}

capture_pty_descendants() {
  PTY_DESCENDANT_PIDS=()
  PTY_DESCENDANT_START=()
  local pid start
  while read -r pid start; do
    [[ -n $pid && -n $start ]] || continue
    PTY_DESCENDANT_PIDS+=("$pid")
    PTY_DESCENDANT_START[$pid]=$start
  done < <(python3 - "$SERVER_PID" <<'PY'
import pathlib, sys
todo = [int(sys.argv[1])]
seen = set(todo)
while todo:
    parent = todo.pop()
    try:
        children = [int(value) for value in pathlib.Path(f"/proc/{parent}/task/{parent}/children").read_text().split()]
    except (OSError, ValueError):
        children = []
    for child in children:
        if child in seen:
            continue
        seen.add(child)
        todo.append(child)
        try:
            stat = pathlib.Path(f"/proc/{child}/stat").read_text()
            start = stat.rsplit(")", 1)[1].split()[19]
        except (IndexError, OSError, ValueError):
            continue
        print(child, start)
PY
  )
}

report_pty_descendant_survivors() {
  local pid survivors=()
  for pid in "${PTY_DESCENDANT_PIDS[@]:-}"; do
    [[ -n $pid ]] && owned_pid_alive "$pid" && survivors+=("$pid")
  done
  ((${#survivors[@]} == 0)) && return 0
  echo "PTY descendants survived session shutdown: ${survivors[*]}" >&2
  return 1
}

cleanup_scenario() {
  set +e
  for socket in "${CONTROL_SOCKETS[@]:-}"; do
    [[ -S $socket ]] && "$BIN" --socket "$socket" run-action detach >/dev/null 2>&1
  done
  if [[ -n ${SESSION:-} ]]; then
    "$BIN" sessions kill "$SESSION" >/dev/null 2>&1
  fi
  local attempt pid
  for attempt in {1..40}; do
    local live=0
    for pid in "${WRAPPER_PIDS[@]:-}" "${CLIENT_PIDS[@]:-}" "${SERVER_PID:-}" "${PTY_DESCENDANT_PIDS[@]:-}"; do
      [[ -n $pid ]] && owned_pid_alive "$pid" && live=1
    done
    ((live == 0)) && break
    sleep 0.05
  done
  for pid in "${WRAPPER_PIDS[@]:-}" "${CLIENT_PIDS[@]:-}" "${SERVER_PID:-}" "${PTY_DESCENDANT_PIDS[@]:-}"; do
    if [[ -n $pid ]] && owned_pid_alive "$pid"; then
      kill "$pid" >/dev/null 2>&1
    fi
  done
  sleep 0.1
  for pid in "${WRAPPER_PIDS[@]:-}" "${CLIENT_PIDS[@]:-}" "${SERVER_PID:-}" "${PTY_DESCENDANT_PIDS[@]:-}"; do
    if [[ -n $pid ]] && owned_pid_alive "$pid"; then
      kill -9 "$pid" >/dev/null 2>&1
    fi
    [[ -n $pid ]] && wait "$pid" >/dev/null 2>&1
  done
  [[ -n ${ROOT:-} ]] && rm -rf -- "$ROOT"
  ROOT= SERVER_PID= SESSION=
  ACTIVE_CLIENTS=0
  WRAPPER_PIDS=()
  CLIENT_PIDS=()
  CONTROL_SOCKETS=()
  PANE_IDS=()
  PANE_MARKERS=()
  PTY_DESCENDANT_PIDS=()
  PTY_DESCENDANT_START=()
  set -e
}
trap cleanup_scenario EXIT INT TERM

wait_for_glob() {
  local pattern=$1 deadline=$((SECONDS + 15)) matches=()
  while ((SECONDS < deadline)); do
    shopt -s nullglob
    matches=($pattern)
    shopt -u nullglob
    ((${#matches[@]} > 0)) && { printf '%s\n' "${matches[@]}"; return 0; }
    sleep 0.05
  done
  echo "timed out waiting for $pattern" >&2
  return 1
}

wait_for_marker() {
  local socket=$1 pane=$2 marker=$3 deadline=$((SECONDS + 20)) capture
  while ((SECONDS < deadline)); do
    if capture=$("$BIN" --socket "$socket" capture-pane --target "$pane" --scrollback full 2>/dev/null) &&
      python3 -c 'import json,sys; text=json.load(sys.stdin)["data"]["text"].replace("\n", ""); sys.exit(0 if sys.argv[1] in text else 1)' "$marker" <<<"$capture"; then
      return 0
    fi
    sleep 0.05
  done
  "$BIN" --socket "$socket" list-panes >&2 || true
  "$BIN" --socket "$socket" capture-pane --target "$pane" --scrollback full >&2 || true
  local log
  for log in "$ROOT"/server.log "$ROOT"/client-*.log; do
    [[ -s $log ]] || continue
    printf '%s:\n' "$log" >&2
    while IFS= read -r line; do printf '  %s\n' "$line" >&2; done <"$log"
  done
  echo "timed out waiting for pane $pane marker $marker" >&2
  return 1
}

wait_for_replay() {
  local socket=$1 index
  for ((index=0; index<${#PANE_IDS[@]}; index++)); do
    wait_for_marker "$socket" "${PANE_IDS[index]}" "${PANE_MARKERS[index]}"
  done
}

wait_for_absent() {
  local path=$1 deadline=$((SECONDS + 15))
  while ((SECONDS < deadline)); do
    [[ ! -e $path && ! -S $path ]] && return 0
    sleep 0.05
  done
  echo "timed out waiting for $path to disappear" >&2
  return 1
}

wait_for_pane() {
  local socket=$1 pane=$2 deadline=$((SECONDS + 15)) panes
  while ((SECONDS < deadline)); do
    if panes=$("$BIN" --socket "$socket" list-panes --format json 2>/dev/null) &&
      python3 -c 'import json,sys; rows=json.load(sys.stdin)["data"]; sys.exit(0 if any(row["id"] == int(sys.argv[1]) for row in rows) else 1)' "$pane" <<<"$panes"; then
      return 0
    fi
    sleep 0.05
  done
  echo "timed out waiting for pane $pane on $socket" >&2
  return 1
}

start_client() {
  local rows=$1 cols=$2 after socket wrapper state status client_pid
  local deadline=$((SECONDS + 15)) log="$ROOT/client-${#WRAPPER_PIDS[@]}.log"
  script -qefc "stty rows $rows cols $cols; exec '$BIN' sessions attach '$SESSION'" /dev/null </dev/null >"$log" 2>&1 &
  wrapper=$!
  WRAPPER_PIDS+=("$wrapper")
  while ((SECONDS < deadline)); do
    shopt -s nullglob
    after=("$XDG_RUNTIME_DIR"/rozi/control-*.sock)
    shopt -u nullglob
    for socket in "${after[@]}"; do
      if [[ ! " ${CONTROL_SOCKETS[*]:-} " =~ " $socket " ]]; then
        CONTROL_SOCKETS+=("$socket")
        client_pid=${socket##*/control-}
        client_pid=${client_pid%.sock}
        [[ $client_pid =~ ^[0-9]+$ ]] && CLIENT_PIDS+=("$client_pid")
        return 0
      fi
    done
    if [[ ! -r /proc/$wrapper/stat ]]; then
      status=0
      wait "$wrapper" || status=$?
      echo "client wrapper $wrapper exited before creating a control socket (status $status)" >&2
      [[ -s $log ]] && while IFS= read -r line; do printf '  %s\n' "$line" >&2; done <"$log"
      return 1
    fi
    read -r _ _ state _ <"/proc/$wrapper/stat"
    if [[ $state == Z || $state == X ]]; then
      status=0
      wait "$wrapper" || status=$?
      echo "client wrapper $wrapper exited before creating a control socket (status $status)" >&2
      [[ -s $log ]] && while IFS= read -r line; do printf '  %s\n' "$line" >&2; done <"$log"
      return 1
    fi
    sleep 0.05
  done
  echo "timed out waiting for client wrapper $wrapper to create a control socket" >&2
  [[ -s $log ]] && while IFS= read -r line; do printf '  %s\n' "$line" >&2; done <"$log"
  return 1
}

pane_command() {
  local history=$1 content=$2 marker=$3 marker_suffix=${3#M}
  if [[ $content == images || $content == image-stress ]]; then
    local width=384 height=256 image_count=8
    if [[ $content == image-stress ]]; then
      width=1536
      height=1024
      image_count=12
    fi
    printf '%s' "python3 -c 'import base64,sys,time; w=$width; h=$height; count=$image_count; marker=\"M\"+\"${marker_suffix}\"; raw=bytes((index * 17 + 23) % 251 for index in range(w * h * 3)); payload=base64.b64encode(raw).decode(); [sys.stdout.write(\"\\x1b_Ga=T,f=24,s=%d,v=%d,t=d,i=%d;%s\\x1b\\\\\\n\" % (w,h,image,payload)) for image in range(1,count+1)]; sys.stdout.write(marker+\"\\n\"); sys.stdout.flush(); time.sleep(3600)'"
  elif [[ $content == styled ]]; then
    printf "i=0; while [ \$i -lt %s ]; do printf '\\033[3%%dmrozi-%%06d styled\\033[0m\\n' \$((\$i%%7+1)) \$i; i=\$((\$i+1)); done; printf 'M%%s\\n' '%s'; while :; do sleep 3600; done" "$history" "$marker_suffix"
  else
    printf "i=0; while [ \$i -lt %s ]; do printf 'rozi-%%06d plain\\n' \$i; i=\$((\$i+1)); done; printf 'M%%s\\n' '%s'; while :; do sleep 3600; done" "$history" "$marker_suffix"
  fi
}

measure_groups() {
  local scenario=$1 rows=$2 cols=$3 panes=$4 history=$5 content=$6 clients=$7 state=$8
  local client_csv server_csv shell_csv active_clients=$ACTIVE_CLIENTS
  client_csv=$(IFS=,; printf '%s' "${CLIENT_PIDS[*]:-}")
  server_csv=$SERVER_PID
  if ((${#PTY_DESCENDANT_PIDS[@]})); then
    local captured=() pid
    for pid in "${PTY_DESCENDANT_PIDS[@]}"; do
      captured+=("$pid@${PTY_DESCENDANT_START[$pid]}")
    done
    shell_csv=$(IFS=,; printf '%s' "${captured[*]}")
  else
    shell_csv=$(python3 - "$SERVER_PID" <<'PY'
import pathlib, sys
todo = [int(sys.argv[1])]
seen = set(todo)
children = []
while todo:
    pid = todo.pop()
    path = pathlib.Path(f"/proc/{pid}/task/{pid}/children")
    try:
        found = [int(value) for value in path.read_text().split()]
    except (OSError, ValueError):
        found = []
    for child in found:
        if child not in seen:
            seen.add(child)
            children.append(child)
            todo.append(child)
print(",".join(map(str, children)))
PY
    )
  fi
  sleep "$SETTLE_SECONDS"
  python3 - "$JSONL" "$scenario" "$rows" "$cols" "$panes" "$history" "$content" "$clients" "$active_clients" "$state" "$SAMPLE_COUNT" "$SAMPLE_INTERVAL" "client=$client_csv" "server=$server_csv" "shell=$shell_csv" <<'PY'
import json, pathlib, statistics, sys, time

out, scenario, rows, cols, panes, history, content, clients, active_clients, state, count, interval, *groups = sys.argv[1:]
group_pids = {}
for item in groups:
    name, raw = item.split("=", 1)
    parsed = []
    for token in raw.split(","):
        if not token:
            continue
        pid, separator, start = token.partition("@")
        parsed.append((int(pid), start if separator else None))
    group_pids[name] = parsed

def same_process(pid, expected_start):
    if expected_start is None:
        return pathlib.Path(f"/proc/{pid}").exists()
    try:
        stat = pathlib.Path(f"/proc/{pid}/stat").read_text()
        return stat.rsplit(")", 1)[1].split()[19] == expected_start
    except (IndexError, OSError, ValueError):
        return False

def proc_metrics(pid):
    values = {key: 0 for key in ("rss_kib", "pss_kib", "anonymous_kib", "private_kib", "file_pss_kib", "threads", "current_rss_kib")}
    fields = {}
    for line in pathlib.Path(f"/proc/{pid}/smaps_rollup").read_text().splitlines():
        if ":" in line:
            name, rest = line.split(":", 1)
            parts = rest.split()
            if len(parts) >= 2 and parts[1] == "kB":
                fields[name] = int(parts[0])
    status = {}
    for line in pathlib.Path(f"/proc/{pid}/status").read_text().splitlines():
        if ":" in line:
            name, rest = line.split(":", 1)
            parts = rest.strip().split()
            if parts:
                status[name] = parts[0]
    values.update(
        rss_kib=fields.get("Rss", 0),
        pss_kib=fields.get("Pss", 0),
        anonymous_kib=fields.get("Anonymous", 0),
        private_kib=fields.get("Private_Clean", 0) + fields.get("Private_Dirty", 0),
        file_pss_kib=fields.get("Pss_File", max(0, fields.get("Pss", 0) - fields.get("Pss_Anon", 0) - fields.get("Pss_Shmem", 0))),
        threads=int(status.get("Threads", 0)),
        current_rss_kib=int(status.get("VmRSS", 0)),
    )
    return values

samples = {name: [] for name in group_pids}
for sample_index in range(int(count)):
    for name, pids in group_pids.items():
        aggregate = {"process_count": 0}
        for pid, expected_start in pids:
            if not same_process(pid, expected_start):
                continue
            try:
                metrics = proc_metrics(pid)
            except (FileNotFoundError, ProcessLookupError):
                continue
            aggregate["process_count"] += 1
            for key, value in metrics.items():
                aggregate[key] = aggregate.get(key, 0) + value
        samples[name].append(aggregate)
    if sample_index + 1 < int(count):
        time.sleep(float(interval))

result = {}
for name, rows_of_values in samples.items():
    keys = rows_of_values[0].keys() if rows_of_values else []
    result[name] = {key: int(statistics.median(row.get(key, 0) for row in rows_of_values)) for key in keys}
record = {
    "scenario": scenario,
    "viewport": {"cols": int(cols), "rows": int(rows)},
    "panes": int(panes),
    "history_lines": int(history),
    "content": content,
    "clients": int(clients),
    "active_clients": int(active_clients),
    "state": state,
    "samples": int(count),
    "sample_interval_ms": round(float(interval) * 1000),
    "groups": result,
}
with open(out, "a", encoding="utf-8") as handle:
    handle.write(json.dumps(record, sort_keys=True) + "\n")
PY
}

run_scenario() {
  local rows=$1 cols=$2 panes=$3 history=$4 content=$5 clients=$6 state=${7:-steady}
  [[ $content =~ ^(plain|styled|images|image-stress)$ ]] ||
    { echo "CONTENT must be plain, styled, images, or image-stress" >&2; return 2; }
  [[ $state =~ ^(steady|closed|disconnected|reconnected|killed)$ ]] ||
    { echo "STATE must be steady, closed, disconnected, reconnected, or killed" >&2; return 2; }
  local label="${cols}x${rows}-p${panes}-h${history}-${content}-c${clients}-${state}"
  echo "Measuring $label" >&2
  cleanup_scenario
  ROOT=$(mktemp -d "${TMPDIR:-/tmp}/rozi-memory.XXXXXX")
  chmod 700 "$ROOT"
  mkdir -p "$ROOT"/{home,config,state,cache,data,runtime,work}
  local command marker=M01
  command=$(pane_command "$history" "$content" "$marker")
  {
    printf '%s\n' '#!/bin/sh'
    printf 'while [ ! -e %q ]; do sleep 0.05; done\n' "$ROOT/work/start"
    printf '%s\n' "$command"
  } >"$ROOT/work/initial-workload.sh"
  chmod 700 "$ROOT/work/initial-workload.sh"
  export HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/config" XDG_STATE_HOME="$ROOT/state"
  export XDG_CACHE_HOME="$ROOT/cache" XDG_DATA_HOME="$ROOT/data"
  export XDG_RUNTIME_DIR="$ROOT/runtime"
  export ROZI_CONFIG="$ROOT/config/config.toml" TERM=xterm-256color LANG=C LC_ALL=C SHELL=/bin/sh
  cat >"$ROZI_CONFIG" <<EOF
shell = ["/bin/sh", "$ROOT/work/initial-workload.sh"]
command_shell = ["/bin/sh", "-c"]
cwd = "$ROOT/work"
scrollback = $history

[shell_integration]
mode = "off"

[session]
autosave = false
resurrect = false

[confirm]
kill_session = false

[animations]
enabled = false
EOF
  SESSION="memory-$RANDOM-$$"
  "$BIN" --session "$SESSION" --fresh-server >"$ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  if ! wait_for_glob "$XDG_RUNTIME_DIR/rozi/session-*.sock" >/dev/null; then
    [[ -s $ROOT/server.log ]] &&
      while IFS= read -r line; do printf '  %s\n' "$line" >&2; done <"$ROOT/server.log"
    return 1
  fi
  local client
  for ((client=0; client<clients; client++)); do start_client "$rows" "$cols"; done
  ACTIVE_CLIENTS=$clients
  local control=${CONTROL_SOCKETS[0]} response pane id probe_socket
  for probe_socket in "${CONTROL_SOCKETS[@]}"; do wait_for_pane "$probe_socket" 1; done
  : >"$ROOT/work/start"
  wait_for_marker "$control" 1 "$marker"
  PANE_IDS=(1)
  PANE_MARKERS=("$marker")
  for ((pane=2; pane<=panes; pane++)); do
    printf -v marker 'M%02d' "$pane"
    command=$(pane_command "$history" "$content" "$marker")
    response=$("$BIN" --socket "$control" split "$command")
    id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["id"])' <<<"$response")
    PANE_IDS+=("$id")
    PANE_MARKERS+=("$marker")
    wait_for_marker "$control" "$id" "$marker"
  done
  for probe_socket in "${CONTROL_SOCKETS[@]}"; do wait_for_replay "$probe_socket"; done
  if [[ $state == closed ]]; then
    for id in "${PANE_IDS[@]:$((panes / 2))}"; do
      "$BIN" --socket "$control" focus "$id" >/dev/null
      "$BIN" --socket "$control" run-action close >/dev/null
    done
    sleep 0.5
  elif [[ $state == disconnected ]]; then
    local disconnected=${CONTROL_SOCKETS[-1]}
    "$BIN" --socket "$disconnected" run-action detach >/dev/null
    wait_for_absent "$disconnected"
    unset 'CONTROL_SOCKETS[-1]'
    CONTROL_SOCKETS=("${CONTROL_SOCKETS[@]}")
    ((ACTIVE_CLIENTS--))
  elif [[ $state == reconnected ]]; then
    for control in "${CONTROL_SOCKETS[@]}"; do
      "$BIN" --socket "$control" run-action detach >/dev/null
      wait_for_absent "$control"
    done
    CONTROL_SOCKETS=()
    for ((client=0; client<clients; client++)); do start_client "$rows" "$cols"; done
    for control in "${CONTROL_SOCKETS[@]}"; do wait_for_replay "$control"; done
    ACTIVE_CLIENTS=$clients
  elif [[ $state == killed ]]; then
    local session_endpoint="$XDG_RUNTIME_DIR/rozi/session-$SESSION.sock"
    capture_pty_descendants
    "$BIN" --socket "$control" run-action kill-session >/dev/null
    wait_for_absent "$session_endpoint"
    ACTIVE_CLIENTS=0
  fi
  CLIENT_PIDS=()
  if ((${#CONTROL_SOCKETS[@]})); then
    mapfile -t CLIENT_PIDS < <(for control in "${CONTROL_SOCKETS[@]}"; do basename "$control" | cut -d- -f2 | cut -d. -f1; done)
  fi
  measure_groups "$label" "$rows" "$cols" "$panes" "$history" "$content" "$clients" "$state"
  if [[ $state == killed ]]; then
    report_pty_descendant_survivors
  fi
  cleanup_scenario
}

run_smoke_matrix() {
  run_scenario 24 80 2 10 images 2 disconnected
  run_scenario 24 80 2 10 images 2 reconnected
  run_scenario 24 80 2 10 images 2 killed
}

run_lifecycle_matrix() {
  run_scenario 64 253 8 5000 images 2 steady
  run_scenario 64 253 8 5000 images 2 closed
  run_scenario 64 253 8 5000 images 2 disconnected
  run_scenario 64 253 8 5000 images 2 reconnected
  run_scenario 64 253 8 5000 images 2 killed
}

if [[ $MODE == case ]]; then
  run_scenario "${CUSTOM_CASE[@]}"
elif [[ $MODE == smoke ]]; then
  run_smoke_matrix
elif [[ $MODE == lifecycle ]]; then
  run_lifecycle_matrix
else
  viewports=("24 80" "64 253")
  panes_values=(1 4 8)
  histories=(0 1000)
  clients_values=(1)
  if [[ $MODE == full ]]; then
    panes_values+=(16)
    histories+=(5000)
    clients_values+=(2)
  fi
  for viewport in "${viewports[@]}"; do
    read -r rows cols <<<"$viewport"
    for panes in "${panes_values[@]}"; do
      for history in "${histories[@]}"; do
        for content in plain styled; do
          for clients in "${clients_values[@]}"; do
            run_scenario "$rows" "$cols" "$panes" "$history" "$content" "$clients"
          done
        done
      done
    done
  done
  if [[ $MODE == full ]]; then
    run_scenario 64 253 16 5000 styled 1 closed
    run_scenario 64 253 16 5000 styled 1 disconnected
    run_scenario 64 253 16 5000 styled 1 reconnected
    run_scenario 64 253 16 5000 styled 1 killed
    run_lifecycle_matrix
  fi
fi

python3 - "$JSONL" "$OUTPUT_DIR/results.json" "$OUTPUT_DIR/results.md" <<'PY'
import json, pathlib, sys
source, json_path, markdown_path = map(pathlib.Path, sys.argv[1:])
rows = [json.loads(line) for line in source.read_text().splitlines() if line]
for row in rows:
    app_pss = sum(group.get("pss_kib", 0) for name, group in row["groups"].items() if name != "shell")
    row["application_pss_kib"] = app_pss

steady = [row for row in rows if row["state"] == "steady"]
slopes = []
keys = lambda row: (row["viewport"]["cols"], row["viewport"]["rows"], row["history_lines"], row["content"], row["clients"])
for current in steady:
    previous = [row for row in steady if keys(row) == keys(current) and row["panes"] < current["panes"]]
    if previous:
        base = max(previous, key=lambda row: row["panes"])
        slopes.append({
            "from_panes": base["panes"], "to_panes": current["panes"],
            "viewport": current["viewport"], "history_lines": current["history_lines"],
            "content": current["content"], "clients": current["clients"],
            "application_pss_kib_per_pane": round((current["application_pss_kib"] - base["application_pss_kib"]) / (current["panes"] - base["panes"]), 1),
        })
document = {"schema_version": 2, "sampling": {"settling_seconds": 2, "samples": 5, "interval_ms": 200, "statistic": "median"}, "scenarios": rows, "derived_per_pane": slopes}
json_path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
lines = ["# rozi memory matrix", "", "Values are median KiB from five samples after quiescence; application PSS and current RSS are the session server plus live probe-client processes. Active clients counts session attachments. VmHWM is deliberately not a cleanup metric.", "", "| Scenario | Active clients | Client PSS | Server PSS | Child PSS | App PSS | Current app RSS | Private | Anonymous | File PSS | Threads | Processes |", "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
for row in rows:
    groups = row["groups"]
    client, server, shell = (groups.get(name, {}) for name in ("client", "server", "shell"))
    app_rss = client.get("current_rss_kib", 0) + server.get("current_rss_kib", 0)
    private = client.get("private_kib", 0) + server.get("private_kib", 0)
    anonymous = client.get("anonymous_kib", 0) + server.get("anonymous_kib", 0)
    file_pss = client.get("file_pss_kib", 0) + server.get("file_pss_kib", 0)
    threads = client.get("threads", 0) + server.get("threads", 0)
    processes = sum(group.get("process_count", 0) for group in groups.values())
    lines.append(f'| `{row["scenario"]}` | {row["active_clients"]} | {client.get("pss_kib", 0)} | {server.get("pss_kib", 0)} | {shell.get("pss_kib", 0)} | {row["application_pss_kib"]} | {app_rss} | {private} | {anonymous} | {file_pss} | {threads} | {processes} |')
lines += ["", "## Per-pane application PSS deltas", "", "| From | To | Viewport | History | Content | Clients | KiB/pane |", "| ---: | ---: | --- | ---: | --- | ---: | ---: |"]
for slope in slopes:
    viewport = slope["viewport"]
    lines.append(f'| {slope["from_panes"]} | {slope["to_panes"]} | {viewport["cols"]}x{viewport["rows"]} | {slope["history_lines"]} | {slope["content"]} | {slope["clients"]} | {slope["application_pss_kib_per_pane"]} |')
markdown_path.write_text("\n".join(lines) + "\n")
PY

rm -f "$JSONL"
echo "Wrote $OUTPUT_DIR/results.json and $OUTPUT_DIR/results.md" >&2
