#!/usr/bin/env bash
# Interleaved vtebench and memory runs of upstream Alacritty against the compact-scrollback branch.
# Only signals the Alacritty processes it starts itself. Resumable: finished runs are skipped.
set -uo pipefail
B=${ALACRITTY_BENCH:?set ALACRITTY_BENCH to the benchmark workspace}
OUT=${OUT:-$B/results}
ROUNDS=${ROUNDS:-3}
# Space-separated vtebench `-b` paths relative to the vtebench checkout; defaults to every benchmark.
BENCHES=${BENCHES:-benchmarks ../extra}
BENCH_ARGS=$(for bench in $BENCHES; do printf -- '-b %s ' "$bench"; done)
mkdir -p "$OUT"
CONFIG=$B/bench.toml
printf '[window]\nstartup_mode = "Fullscreen"\n[scrolling]\nhistory = 10000\n' >"$CONFIG"

binary() { case $1 in upstream) echo "$B/bin/alacritty-upstream" ;; *) echo "$B/bin/alacritty-branch" ;; esac; }

# Run one Alacritty with a command; wait for it to exit, or kill it after $2 seconds.
run_alacritty() { # mode timeout command
  local mode=$1 limit=$2 cmd=$3 exe pid started
  exe=$(binary "$mode")
  if [[ $mode == compact ]]; then
    ALACRITTY_COMPACT_SCROLLBACK=1 "$exe" --config-file "$CONFIG" -e sh -c "$cmd" &
  else
    env -u ALACRITTY_COMPACT_SCROLLBACK "$exe" --config-file "$CONFIG" -e sh -c "$cmd" &
  fi
  pid=$!
  echo "$pid" >"$OUT/current.pid"
  started=$SECONDS
  while kill -0 "$pid" 2>/dev/null; do
    if ((SECONDS - started > limit)); then
      [[ $(readlink "/proc/$pid/exe") == "$exe" ]] && kill "$pid"
      echo "timeout $mode" >&2
      break
    fi
    sleep 1
  done
  wait "$pid" 2>/dev/null
}

wait_idle() {
  until [ "$(awk '{print int($1)}' /proc/loadavg)" -lt 2 ] && ! pgrep -x rustc >/dev/null; do sleep 5; done
}

for round in $(seq "$ROUNDS"); do
  for mode in upstream compact dense; do
    [[ -s $OUT/vte-$mode-$round.dat ]] && continue
    wait_idle
    echo "round $round $mode vtebench $(date +%T)" >&2
    run_alacritty "$mode" 900 "sleep 1; cd $B/vtebench && tput cols > $OUT/size-$mode-$round && tput lines >> $OUT/size-$mode-$round && ./target/release/vtebench -s --max-secs 5 $BENCH_ARGS--dat $OUT/vte-$mode-$round.dat"
  done
done

# Memory: fill 10,000 lines of history with short or full-width lines, then sample PSS while idle.
[[ -n ${SKIP_MEMORY:-} ]] && { echo DONE >&2; exit 0; }
for round in $(seq "$ROUNDS"); do
  for content in short full; do
    for mode in upstream compact dense; do
      grep -q "^$round $content $mode " "$OUT/memory.txt" 2>/dev/null && continue
      wait_idle
      rm -f "$OUT/ready"
      if [[ $content == short ]]; then
        gen='i=0; while [ $i -lt 12000 ]; do printf "2026-09-15 INFO request %05d ok\n" $i; i=$((i+1)); done'
      else
        gen='cols=$(tput cols); i=0; while [ $i -lt 12000 ]; do tr -dc "a-zA-Z0-9" </dev/urandom | head -c $cols; printf "\n"; i=$((i+1)); done'
      fi
      echo "round $round $mode memory $content $(date +%T)" >&2
      (run_alacritty "$mode" 300 "sleep 1; $gen; : > $OUT/ready; sleep 12") &
      runner=$!
      until [[ -e $OUT/ready ]] || ! kill -0 "$runner" 2>/dev/null; do sleep 0.5; done
      sleep 4
      pid=$(cat "$OUT/current.pid")
      if [[ -r /proc/$pid/smaps_rollup ]]; then
        pss=$(awk '/^Pss:/{print $2}' "/proc/$pid/smaps_rollup")
        anon=$(awk '/^Pss_Anon:/{print $2}' "/proc/$pid/smaps_rollup")
        rss=$(awk '/^VmRSS:/{print $2}' "/proc/$pid/status")
        echo "$round $content $mode pss_kib=$pss pss_anon_kib=$anon rss_kib=$rss" >>"$OUT/memory.txt"
      fi
      wait "$runner"
    done
  done
done
echo DONE >&2
