#!/usr/bin/env python3
"""Write the two ingest corpora: 2M short plain lines and 1M styled log lines."""

import sys
from pathlib import Path

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)
with open(out / "plain.txt", "w") as f:
    for i in range(2_000_000):
        f.write(f"rozi-{i:07d} plain\n")
levels = [("32", "INFO "), ("33", "WARN "), ("36", "DEBUG"), ("31", "ERROR")]
with open(out / "log.txt", "w") as f:
    for i in range(1_000_000):
        color, level = levels[i % 7 % 4]
        f.write(
            f"\x1b[2m2026-09-14T12:{i // 60000 % 60:02d}:{i // 1000 % 60:02d}.{i % 1000:03d}Z\x1b[0m "
            f"\x1b[{color}m{level}\x1b[0m http: GET /api/items/{i % 9973} status=200 dur={i % 97}ms\n"
        )
