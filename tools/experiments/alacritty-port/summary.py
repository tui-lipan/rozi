#!/usr/bin/env python3
"""Median ms per vtebench sample per mode, and median PSS, from results/."""
import collections, glob, os, re, statistics as st, sys

out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(os.path.dirname(__file__), "results")
per = collections.defaultdict(lambda: collections.defaultdict(list))
for path in sorted(glob.glob(os.path.join(out, "vte-*.dat"))):
    mode = re.match(r"vte-(\w+)-\d+\.dat", os.path.basename(path)).group(1)
    lines = [l for l in open(path).read().split("\n") if l.strip()]
    names = lines[0].split()
    columns = collections.defaultdict(list)
    for row in lines[1:]:
        for name, value in zip(names, row.split()):
            if value != "_":  # benchmarks with fewer samples pad their column
                columns[name].append(float(value))
    for name, values in columns.items():
        per[name][mode].append(st.median(values))

print(f"{'benchmark':30} {'upstream':>9} {'dense':>16} {'compact':>16}")
for name in sorted(per):
    m = per[name]
    up = st.median(m["upstream"]) if m.get("upstream") else None
    cells = [f"{up:9.1f}" if up else f"{'-':>9}"]
    for mode in ("dense", "compact"):
        if m.get(mode):
            v = st.median(m[mode])
            rel = f"({(v / up - 1) * 100:+.0f}%)" if up else ""
            cells.append(f"{v:7.1f} {rel:>8}")
        else:
            cells.append(f"{'-':>16}")
    print(f"{name:30} " + " ".join(cells) + f"   rounds={ {k: [round(x, 1) for x in v] for k, v in m.items()} }")

mem = os.path.join(out, "memory.txt")
if os.path.exists(mem):
    rows = collections.defaultdict(list)
    for line in open(mem):
        parts = line.split()
        content, mode = parts[1], parts[2]
        rows[(content, mode)].append({k: int(v) for k, v in (p.split("=") for p in parts[3:])})
    print("\nmemory, MiB (median of rounds)")
    for content in ("short", "full"):
        for mode in ("upstream", "dense", "compact"):
            r = rows.get((content, mode))
            if r:
                med = lambda key: st.median(x[key] for x in r) / 1024
                print(f"  {content:5} {mode:8} PSS {med('pss_kib'):7.1f}  anon {med('pss_anon_kib'):7.1f}  "
                      f"RSS {med('rss_kib'):7.1f}  runs={[round(x['pss_kib'] / 1024, 1) for x in r]}")
