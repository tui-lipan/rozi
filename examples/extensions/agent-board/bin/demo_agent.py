#!/usr/bin/env python3
"""A pretend coding agent, so the board has something to show.

Run it in a pane. It works for a few seconds, asks before applying its change, and waits for the
next prompt. The `[[agents]]` rules in extension.toml read these screens.
"""

from __future__ import annotations

import time

PROMPT = "› "
WORKING = "Thinking… (esc to interrupt)"
ASKING = "Apply this change? (y/n) "


def main() -> int:
    print("Demo agent. Type a request, or Ctrl+D to quit.")
    while True:
        try:
            request = input(PROMPT)
        except EOFError:
            return 0
        if not request.strip():
            continue
        print(WORKING, flush=True)
        time.sleep(4)
        try:
            answer = input(ASKING)
        except EOFError:
            return 0
        print("Applied." if answer.strip().lower() == "y" else "Left unchanged.")


if __name__ == "__main__":
    raise SystemExit(main())
