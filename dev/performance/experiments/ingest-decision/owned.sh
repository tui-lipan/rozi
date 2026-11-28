# Sourced helpers: only ever signal processes this harness launched.
# A PID is "owned" when its executable is one of the experiment binaries in $EXP_DIR (named
# rozi-*), or the util-linux `script` wrapper it started, AND its start time matches what was
# recorded. Developers usually run their own Rozi on the same machine; never signal by name.
EXP_DIR=${EXP_DIR:?set EXP_DIR to the directory holding rozi-dense and rozi-compact}

proc_start() { sed 's/.*) //' "/proc/$1/stat" 2>/dev/null | awk '{print $20}'; }

declare -A OWNED_START=()

own() { # pid
  local pid=$1 exe attempt
  # `$!` names the forked shell until it execs, so give the exec a moment before judging the binary.
  # Refusing too early made a caller exit without cleanup and leave its processes running.
  for attempt in $(seq 50); do
    exe=$(readlink "/proc/$pid/exe" 2>/dev/null) || { echo "own: $pid has no exe" >&2; return 1; }
    case $exe in "$EXP_DIR"/rozi-*|/usr/bin/script) break ;; esac
    sleep 0.02
  done
  case $exe in
    "$EXP_DIR"/rozi-*|/usr/bin/script) ;;
    *) echo "own: refusing $pid ($exe)" >&2; return 1 ;;
  esac
  OWNED_START[$pid]=$(proc_start "$pid")
}

owned_alive() { # pid
  local pid=$1
  [[ -n ${OWNED_START[$pid]+x} ]] || return 1
  [[ $(proc_start "$pid") == "${OWNED_START[$pid]}" ]]
}

owned_signal() { # sig pid...
  local sig=$1 pid; shift
  for pid in "$@"; do
    [[ -n $pid ]] || continue
    if [[ -z ${OWNED_START[$pid]+x} ]]; then
      echo "owned_signal: $pid was never owned; not signalling" >&2
      continue
    fi
    owned_alive "$pid" && kill "-$sig" "$pid" 2>/dev/null
  done
  return 0
}

# Terminate owned pids, escalating, and fail if any survive.
owned_reap() { # pid...
  local pid attempt live
  owned_signal TERM "$@"
  for attempt in $(seq 40); do
    live=0
    for pid in "$@"; do [[ -n $pid ]] && owned_alive "$pid" && live=1; done
    ((live == 0)) && return 0
    sleep 0.05
  done
  owned_signal KILL "$@"
  sleep 0.2
  for pid in "$@"; do
    [[ -n $pid ]] && owned_alive "$pid" && { echo "owned_reap: $pid survived SIGKILL" >&2; return 1; }
  done
  return 0
}

# Every experiment binary still running anywhere on the machine (by exe path, never by name).
experiment_survivors() {
  local p exe
  for p in /proc/[0-9]*; do
    exe=$(readlink "$p/exe" 2>/dev/null) || continue
    [[ $exe == "$EXP_DIR"/rozi-* ]] && echo "${p#/proc/} $exe"
  done
  return 0
}
