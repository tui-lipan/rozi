#!/usr/bin/env python3
"""Say hello through Rozi, optionally to someone chosen in a picker."""

from __future__ import annotations

import json
import os
import subprocess
import sys

ROZI = os.environ.get("ROZI_BIN", "rozi")
NAMES = ["world", "Rozi", "future me"]


def greeting(name: str) -> str:
    return f"Hello, {name}!"


def picker_document(names: list[str]) -> str:
    """The first and only line `rozi pick --json` reads: the picker and its rows."""
    return json.dumps(
        {
            "title": "Say hello to",
            "rows": [{"id": name, "label": name} for name in names],
        }
    )


def chosen(output: str) -> str | None:
    """The selected row, or None when the user cancelled."""
    for line in output.splitlines():
        try:
            reply = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(reply, dict) and isinstance(reply.get("selected"), str):
            return reply["selected"]
    return None


def pick_name() -> str | None:
    result = subprocess.run(
        [ROZI, "pick", "--json"],
        input=picker_document(NAMES) + "\n",
        capture_output=True,
        text=True,
        check=False,
    )
    return chosen(result.stdout) if result.returncode == 0 else None


def main(arguments: list[str]) -> int:
    if os.environ.get("ROZI_EXTENSION") != "hello":
        print("run this through rozi: rozi run-action hello.greet", file=sys.stderr)
        return 2
    name = pick_name() if "--pick" in arguments else NAMES[0]
    if name is None:
        return 0
    return subprocess.run([ROZI, "notify", greeting(name)], check=False).returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
