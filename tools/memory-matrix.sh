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
Usage: tools/memory-matrix.sh [--quick|--full|--smoke] [--case ROWS COLS PANES HISTORY CONTENT CLIENTS] [--output DIR]

Linux-only, opt-in PSS benchmark. --quick is the default; --smoke runs one
scenario to validate the local dependencies and lifecycle without measuring a matrix.
EOF
}

while (($#)); do
  case "$1" in
    --quick|--full|--smoke) MODE=${1#--}; shift ;;
    --case)
      (($# >= 7)) || { echo "--case requires ROWS COLS PANES HISTORY CONTENT CLIENTS" >&2; exit 2; }
      MODE=case
      CUSTOM_CASE=("$2" "$3" "$4" "$5" "$6" "$7")
      shift 7
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
BIN="$REPO/target/release/hyprmux"

ROOT=
SERVER_PID=
WRAPPER_PIDS=()
SESSION=
CONTROL_SOCKETS=()

cleanup_scenario() {
  set +e
  for socket in "${CONTROL_SOCKETS[@]:-}"; do
    [[ -S $socket ]] && "$BIN" --socket "$socket" run-action detach >/dev/null 2>&1
  done
  if [[ -n ${SESSION:-} ]]; then
    "$BIN" kill-session "$SESSION" >/dev/null 2>&1
  fi
  local attempt pid
  for attempt in {1..40}; do
    local live=0
    for pid in "${WRAPPER_PIDS[@]:-}" "${SERVER_PID:-}"; do
      [[ -n $pid ]] && kill -0 "$pid" >/dev/null 2>&1 && live=1
    done
    ((live == 0)) && break
    sleep 0.05
  done
  for pid in "${WRAPPER_PIDS[@]:-}" "${SERVER_PID:-}"; do
    if [[ -n $pid ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill "$pid" >/dev/null 2>&1
    fi
  done
  sleep 0.1
  for pid in "${WRAPPER_PIDS[@]:-}" "${SERVER_PID:-}"; do
    if [[ -n $pid ]] && kill -0 "$pid" >/dev/null 2>&1; then
      kill -9 "$pid" >/dev/null 2>&1
    fi
    [[ -n $pid ]] && wait "$pid" >/dev/null 2>&1
  done
  [[ -n ${ROOT:-} ]] && rm -rf -- "$ROOT"
  ROOT= SERVER_PID= SESSION=
  WRAPPER_PIDS=()
  CONTROL_SOCKETS=()
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
  echo "timed out waiting for pane $pane marker $marker" >&2
  return 1
}

send_when_ready() {
  local socket=$1 text=$2 deadline=$((SECONDS + 15))
  while ((SECONDS < deadline)); do
    if "$BIN" --socket "$socket" send-text "$text" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.05
  done
  echo "timed out waiting for the initial pane PTY" >&2
  return 1
}

start_client() {
  local rows=$1 cols=$2 before after socket wrapper
  before=${#CONTROL_SOCKETS[@]}
  script -qefc "stty rows $rows cols $cols; exec '$BIN' attach '$SESSION'" /dev/null </dev/null >/dev/null 2>&1 &
  wrapper=$!
  WRAPPER_PIDS+=("$wrapper")
  while :; do
    mapfile -t after < <(wait_for_glob "$XDG_RUNTIME_DIR/hyprmux/control-*.sock")
    if ((${#after[@]} > before)); then
      for socket in "${after[@]}"; do
        if [[ ! " ${CONTROL_SOCKETS[*]:-} " =~ " $socket " ]]; then
          CONTROL_SOCKETS+=("$socket")
          return 0
        fi
      done
    fi
    sleep 0.05
  done
}

pane_command() {
  local history=$1 content=$2 marker=$3 marker_suffix=${3#M}
  if [[ $content == styled ]]; then
    printf "i=0; while [ \$i -lt %s ]; do printf '\\033[3%%dmhyprmux-%%06d styled\\033[0m\\n' \$((\$i%%7+1)) \$i; i=\$((\$i+1)); done; printf 'M%%s\\n' '%s'; while :; do sleep 3600; done" "$history" "$marker_suffix"
  else
    printf "i=0; while [ \$i -lt %s ]; do printf 'hyprmux-%%06d plain\\n' \$i; i=\$((\$i+1)); done; printf 'M%%s\\n' '%s'; while :; do sleep 3600; done" "$history" "$marker_suffix"
  fi
}

measure_groups() {
  local scenario=$1 rows=$2 cols=$3 panes=$4 history=$5 content=$6 clients=$7 state=$8
  local client_csv server_csv shell_csv
  client_csv=$(IFS=,; printf '%s' "${CLIENT_PIDS[*]:-}")
  server_csv=$SERVER_PID
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
  sleep "$SETTLE_SECONDS"
  python3 - "$JSONL" "$scenario" "$rows" "$cols" "$panes" "$history" "$content" "$clients" "$state" "$SAMPLE_COUNT" "$SAMPLE_INTERVAL" "client=$client_csv" "server=$server_csv" "shell=$shell_csv" <<'PY'
import json, pathlib, statistics, sys, time

out, scenario, rows, cols, panes, history, content, clients, state, count, interval, *groups = sys.argv[1:]
group_pids = {}
for item in groups:
    name, raw = item.split("=", 1)
    group_pids[name] = [int(pid) for pid in raw.split(",") if pid and pathlib.Path(f"/proc/{pid}").exists()]

def proc_metrics(pid):
    values = {key: 0 for key in ("rss_kib", "pss_kib", "anonymous_kib", "private_kib", "file_pss_kib", "threads", "current_rss_kib", "high_water_rss_kib")}
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
        high_water_rss_kib=int(status.get("VmHWM", 0)),
    )
    return values

samples = {name: [] for name in group_pids}
for sample_index in range(int(count)):
    for name, pids in group_pids.items():
        aggregate = {"process_count": 0}
        for pid in pids:
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
  local label="${cols}x${rows}-p${panes}-h${history}-${content}-c${clients}-${state}"
  echo "Measuring $label" >&2
  cleanup_scenario
  ROOT=$(mktemp -d "${TMPDIR:-/tmp}/hyprmux-memory.XXXXXX")
  chmod 700 "$ROOT"
  mkdir -p "$ROOT"/{home,config,state,cache,runtime,work}
  export HOME="$ROOT/home" XDG_CONFIG_HOME="$ROOT/config" XDG_STATE_HOME="$ROOT/state"
  export XDG_CACHE_HOME="$ROOT/cache" XDG_RUNTIME_DIR="$ROOT/runtime"
  export HYPRMUX_CONFIG="$ROOT/config/hyprmux.toml" TERM=xterm-256color LANG=C LC_ALL=C SHELL=/bin/sh
  cat >"$HYPRMUX_CONFIG" <<EOF
shell = ["/bin/sh"]
command_shell = ["/bin/sh", "-c"]
cwd = "$ROOT/work"
scrollback = $history

[shell_integration]
mode = "off"

[session]
autosave = false
resurrect = false

[animations]
enabled = false
EOF
  SESSION="memory-$RANDOM-$$"
  "$BIN" --session "$SESSION" --fresh-server >"$ROOT/server.log" 2>&1 &
  SERVER_PID=$!
  wait_for_glob "$XDG_RUNTIME_DIR/hyprmux/session-*.sock" >/dev/null
  local client
  for ((client=0; client<clients; client++)); do start_client "$rows" "$cols"; done
  local control=${CONTROL_SOCKETS[0]} command marker response pane id
  marker=M01
  command=$(pane_command "$history" "$content" "$marker")
  send_when_ready "$control" "$command"$'\n'
  wait_for_marker "$control" 1 "$marker"
  PANE_IDS=(1)
  for ((pane=2; pane<=panes; pane++)); do
    printf -v marker 'M%02d' "$pane"
    command=$(pane_command "$history" "$content" "$marker")
    response=$("$BIN" --socket "$control" new-pane "$command")
    id=$(python3 -c 'import json,sys; print(json.load(sys.stdin)["data"]["id"])' <<<"$response")
    PANE_IDS+=("$id")
    wait_for_marker "$control" "$id" "$marker"
  done
  if [[ $state == closed ]]; then
    for id in "${PANE_IDS[@]:$((panes / 2))}"; do
      "$BIN" --socket "$control" focus "$id" >/dev/null
      "$BIN" --socket "$control" run-action close >/dev/null
    done
    sleep 0.5
  elif [[ $state == parked ]]; then
    for control in "${CONTROL_SOCKETS[@]}"; do
      "$BIN" --socket "$control" run-action detach >/dev/null
    done
    sleep 0.5
    CONTROL_SOCKETS=()
  fi
  CLIENT_PIDS=()
  if ((${#CONTROL_SOCKETS[@]})); then
    mapfile -t CLIENT_PIDS < <(for control in "${CONTROL_SOCKETS[@]}"; do basename "$control" | cut -d- -f2 | cut -d. -f1; done)
  fi
  measure_groups "$label" "$rows" "$cols" "$panes" "$history" "$content" "$clients" "$state"
  cleanup_scenario
}

if [[ $MODE == case ]]; then
  run_scenario "${CUSTOM_CASE[@]}"
elif [[ $MODE == smoke ]]; then
  run_scenario 24 80 1 10 plain 1
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
    run_scenario 64 253 16 5000 styled 1 parked
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
document = {"schema_version": 1, "sampling": {"settling_seconds": 2, "samples": 5, "interval_ms": 200, "statistic": "median"}, "scenarios": rows, "derived_per_pane": slopes}
json_path.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n")
lines = ["# hyprmux memory matrix", "", "Values are median KiB from five samples; application PSS is server plus attached clients.", "", "| Scenario | Client PSS | Server PSS | Child PSS | App PSS | App RSS | Private | Anonymous | File PSS | Threads | Processes |", "| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |"]
for row in rows:
    groups = row["groups"]
    client, server, shell = (groups.get(name, {}) for name in ("client", "server", "shell"))
    app_rss = client.get("rss_kib", 0) + server.get("rss_kib", 0)
    private = client.get("private_kib", 0) + server.get("private_kib", 0)
    anonymous = client.get("anonymous_kib", 0) + server.get("anonymous_kib", 0)
    file_pss = client.get("file_pss_kib", 0) + server.get("file_pss_kib", 0)
    threads = client.get("threads", 0) + server.get("threads", 0)
    processes = sum(group.get("process_count", 0) for group in groups.values())
    lines.append(f'| `{row["scenario"]}` | {client.get("pss_kib", 0)} | {server.get("pss_kib", 0)} | {shell.get("pss_kib", 0)} | {row["application_pss_kib"]} | {app_rss} | {private} | {anonymous} | {file_pss} | {threads} | {processes} |')
lines += ["", "## Per-pane application PSS deltas", "", "| From | To | Viewport | History | Content | Clients | KiB/pane |", "| ---: | ---: | --- | ---: | --- | ---: | ---: |"]
for slope in slopes:
    viewport = slope["viewport"]
    lines.append(f'| {slope["from_panes"]} | {slope["to_panes"]} | {viewport["cols"]}x{viewport["rows"]} | {slope["history_lines"]} | {slope["content"]} | {slope["clients"]} | {slope["application_pss_kib_per_pane"]} |')
markdown_path.write_text("\n".join(lines) + "\n")
PY

rm -f "$JSONL"
echo "Wrote $OUTPUT_DIR/results.json and $OUTPUT_DIR/results.md" >&2
