#!/usr/bin/env python3
"""clip.py SOCKET COLS ROWS SETTLE_MS STEPS -- COMMAND...

Runs COMMAND in a private tmux server at SOCKET, in a terminal of COLS x ROWS, waits SETTLE_MS,
then plays STEPS in real time and stops the server. The clip itself is whatever the steps record,
normally with `rozi record start ui`. tmux stands in for a terminal emulator: through a bare
pseudo-terminal, keys reach the focused pane but never rozi's own bindings.

STEPS are separated by newlines:

  key:NAME      one key in tmux's notation: Enter, Escape, C-a, M-l, Up
  type:TEXT     literal text, one character every 45 ms
  sleep:MS      wait
  run:COMMAND   run a shell command, such as `rozi record start ui`, and wait for it
"""
import subprocess
import sys
import time

TYPE_DELAY = 0.045


def main() -> None:
    argv = sys.argv[1:]
    if "--" not in argv or argv.index("--") != 5:
        sys.exit(__doc__)
    socket, cols, rows, settle_ms, steps = argv[:5]
    command = argv[6:]

    def tmux(*args: str) -> None:
        subprocess.run(["tmux", "-S", socket, *args], check=True)

    # No config file, and rozi sees the terminal it would outside tmux.
    tmux("-f", "/dev/null", "new-session", "-d", "-x", cols, "-y", rows,
         "env", "-u", "TMUX", "-u", "TMUX_PANE", "TERM=xterm-256color", *command)
    try:
        time.sleep(int(settle_ms) / 1000)
        for step in steps.splitlines():
            step = step.strip()
            if not step:
                continue
            kind, _, arg = step.partition(":")
            if kind == "key":
                tmux("send-keys", arg.strip())
            elif kind == "type":
                for ch in arg:
                    tmux("send-keys", "-l", ch)
                    time.sleep(TYPE_DELAY)
            elif kind == "sleep":
                time.sleep(int(arg) / 1000)
            elif kind == "run":
                subprocess.run(arg, shell=True, check=True)
            else:
                sys.exit(f"clip.py: unknown step {step!r}")
    finally:
        subprocess.run(["tmux", "-S", socket, "kill-server"], check=False)


if __name__ == "__main__":
    main()
