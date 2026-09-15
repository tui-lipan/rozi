#!/usr/bin/env bash
# Interleaved Criterion A/B of benches/scrollback.rs across prebuilt bench binaries.
# Usage: crit-ab.sh ROUNDS FILTER LABEL=BINARY...
set -euo pipefail
rounds=$1
filter=$2
shift 2
out=${ALACRITTY_BENCH:?set ALACRITTY_BENCH to the benchmark workspace}/crit-$(date +%H%M%S)
mkdir -p "$out"
cd "$out"
until [ "$(awk '{print int($1)}' /proc/loadavg)" -lt 2 ] && ! pgrep -x rustc >/dev/null; do sleep 5; done
for round in $(seq "$rounds"); do
  for pair in "$@"; do
    label=${pair%%=*}
    exe=${pair#*=}
    "$exe" --bench "$filter" --warm-up-time 1 --measurement-time 4 --noplot >"$out/$label-$round.txt" 2>&1
  done
done
python3 - "$out" <<'PY'
import collections, os, re, statistics as st, sys
d = collections.defaultdict(list)
for f in sorted(x for x in os.listdir(sys.argv[1]) if x.endswith(".txt")):
    label = f.rsplit('-', 1)[0]
    name = None
    for line in open(os.path.join(sys.argv[1], f)):
        m = re.match(r'Benchmarking (\S+): Analyzing', line)
        if m:
            name = m.group(1)
        m = re.search(r'time:\s+\[\S+ \S+ (\S+) (\S+) \S+ \S+\]', line)
        if m and name:
            value = float(m.group(1)) * {'ms': 1000, 'µs': 1, 's': 1e6, 'ns': 1e-3}[m.group(2)]
            d[(name.rsplit('/', 1)[0], f"{label}:{name.rsplit('/', 1)[1]}")].append(value)
            name = None
cases = collections.defaultdict(dict)
for (case, key), values in d.items():
    cases[case][key] = (st.median(values), values)
for case in sorted(cases):
    entries = cases[case]
    base = next((v[0] for k, v in entries.items() if k.endswith(':upstream')), None)
    print(case)
    for key in sorted(entries):
        med, values = entries[key]
        rel = f" ({(med / base - 1) * 100:+.1f}%)" if base else ""
        print(f"  {key:28} {med:10.1f}{rel}  runs={[round(v, 1) for v in values]}")
PY
