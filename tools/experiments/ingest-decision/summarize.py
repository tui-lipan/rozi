import json, statistics, sys
from collections import defaultdict

rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
failed = [r for r in rows if r.get("failed")]
groups = defaultdict(list)
for r in rows:
    if not r.get("failed"):
        groups[(r["panes"], r["content"], r["clients"], r["label"])].append(r)

def med(rs, k):
    return statistics.median(r[k] for r in rs)

print("| Panes | Content | Clients | n | Server CPU s d/c | Client CPU s d/c | App CPU Δ | Quiet wall s d/c | Server PSS MiB d/c | Client PSS MiB d/c | App PSS saved MiB |")
print("| ---: | --- | ---: | ---: | --- | --- | ---: | --- | --- | --- | ---: |")
keys = sorted({k[:3] for k in groups}, key=lambda k: (k[1], k[2], k[0]))
for k in keys:
    d, c = groups.get(k + ("dense",), []), groups.get(k + ("compact",), [])
    if not d or not c:
        continue
    ds, cs = med(d, "server_cpu_s"), med(c, "server_cpu_s")
    dc, cc = med(d, "client_cpu_s"), med(c, "client_cpu_s")
    app_d, app_c = ds + dc, cs + cc
    dw, cw = med(d, "quiet_wall_s"), med(c, "quiet_wall_s")
    sp = lambda rs: med(rs, "server_pss_kib") / 1024
    cp = lambda rs: med(rs, "client_pss_kib") / 1024
    saved = sp(d) + cp(d) - sp(c) - cp(c)
    lines = d[0]["lines_per_pane"] * k[0]
    print(f"| {k[0]} | {k[1]} | {k[2]} | {len(d)}/{len(c)} | {ds:.2f}/{cs:.2f} | {dc:.2f}/{cc:.2f} | "
          f"{(app_c - app_d) * 1000:+.0f} ms ({(app_c / app_d - 1) * 100:+.1f}%) | {dw:.2f}/{cw:.2f} | "
          f"{sp(d):.1f}/{sp(c):.1f} | {cp(d):.1f}/{cp(c):.1f} | {saved:.1f} |")
    spread = lambda rs, key: f"{min(r[key] for r in rs):.2f}-{max(r[key] for r in rs):.2f}"
    print(f"<!-- {k}: lines={lines} server d {spread(d,'server_cpu_s')} c {spread(c,'server_cpu_s')}; client d {spread(d,'client_cpu_s')} c {spread(c,'client_cpu_s')} -->")
if failed:
    print(f"\nFAILED runs: {len(failed)}: {failed}")
