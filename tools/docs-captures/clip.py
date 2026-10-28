#!/usr/bin/env python3
"""clip.py TERMINAL_DIR COLS ROWS SETTLE_MS STEPS -- COMMAND...

Runs COMMAND in a pane of COLS x ROWS, waits SETTLE_MS, then plays STEPS in real time and stops
it. The clip itself is whatever the steps record, normally with `rozi record start ui`.

The pane belongs to a detached rozi session whose config, state, and runtime directories are under
TERMINAL_DIR, so the rozi that COMMAND starts never lists it. That session is the terminal emulator:
through a bare pseudo-terminal, keys reach the focused pane but never rozi's own bindings.

STEPS are separated by newlines:

  key:NAME      one key as `rozi send-keys` names it: Enter, Escape, C-a, M-l, Up
  type:TEXT     literal text, one character every 45 ms
  mouse:KIND,X,Y  left-button down, drag, or up at zero-based terminal cells
  float:X,Y,MS  Alt-drag the floating pane on screen so its centre lands on cell X,Y over MS
  sleep:MS      wait
  run:COMMAND   run a shell command, such as `rozi record start ui`, and wait for it
"""
import json
import os
import subprocess
import sys
import time

TYPE_DELAY = 0.045


def main() -> None:
    argv = sys.argv[1:]
    if "--" not in argv or argv.index("--") != 5:
        sys.exit(__doc__)
    terminal_dir, cols, rows, settle_ms, steps = argv[:5]
    command = argv[6:]

    xdg = ("XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME", "XDG_RUNTIME_DIR")
    terminal_env = dict(os.environ)
    for name in xdg:
        terminal_env[name] = os.path.join(terminal_dir, name.lower())
    os.makedirs(terminal_env["XDG_RUNTIME_DIR"], mode=0o700, exist_ok=True)

    def terminal(*args: str) -> str:
        return subprocess.run(["rozi", *args], env=terminal_env, check=True,
                              capture_output=True, text=True).stdout

    def send(*args: str) -> None:
        terminal("--session", "clip-terminal", *args[:1], "--target", pane, *args[1:])

    def mouse(code: int, col: int, row: int, release: bool = False) -> None:
        # Raw bytes, as a terminal reports a mouse event to the program running in it.
        send("send-text", f"\x1b[<{code};{col + 1};{row + 1}{'m' if release else 'M'}")

    def drag_float(col: int, row: int, ms: int) -> None:
        layout = json.loads(subprocess.run(["rozi", "layout", "get", "--format", "json"],
                                           capture_output=True, text=True, check=True).stdout)["data"]
        active = layout["client"]["active_workspace"]
        pane = next(p for w in layout["workspaces"] if w["index"] == active
                    for p in w["panes"] if p["floating"])
        rect = pane["view_rect"]
        x0, y0 = rect["x"] + rect["width"] // 2, rect["y"] + rect["height"] // 2
        alt = 8
        mouse(alt, x0, y0)
        steps = max(2, ms // 33)
        for i in range(1, steps + 1):
            time.sleep(ms / 1000 / steps)
            # Eased in and out, so the drag moves like a hand rather than at constant speed.
            t = i / steps
            t = t * t * (3 - 2 * t)
            mouse(32 + alt, round(x0 + (col - x0) * t), round(y0 + (row - y0) * t))
        mouse(alt, col, row, release=True)

    terminal("sessions", "new", "clip-terminal", "--detach")
    # COMMAND gets the caller's directories back, and not the pane's own ROZI variables, which would
    # make the rozi it starts a client of the terminal session.
    restore = [f"{name}={os.environ[name]}" for name in xdg if name in os.environ]
    pane = str(json.loads(terminal(
        "--session", "clip-terminal", "split", "--size", f"{cols}x{rows}", "--argv",
        "env", *restore, "TERM=xterm-256color",
        "bash", "-c", 'unset "${!ROZI@}"; exec "$@"', "bash", *command,
    ))["data"]["id"])
    try:
        time.sleep(int(settle_ms) / 1000)
        for step in steps.splitlines():
            step = step.strip()
            if not step:
                continue
            kind, _, arg = step.partition(":")
            if kind == "key":
                send("send-keys", arg.strip())
            elif kind == "type":
                for ch in arg:
                    send("send-text", ch)
                    time.sleep(TYPE_DELAY)
            elif kind == "mouse":
                action, col, row = arg.split(",")
                col, row = int(col), int(row)
                if col < 0 or row < 0 or action not in {"down", "drag", "up"}:
                    sys.exit(f"clip.py: invalid mouse step {step!r}")
                mouse(32 if action == "drag" else 0, col, row, release=action == "up")
            elif kind == "float":
                col, row, ms = (int(v) for v in arg.split(","))
                drag_float(col, row, ms)
            elif kind == "sleep":
                time.sleep(int(arg) / 1000)
            elif kind == "run":
                subprocess.run(arg, shell=True, check=True)
            else:
                sys.exit(f"clip.py: unknown step {step!r}")
    finally:
        subprocess.run(["rozi", "sessions", "kill", "clip-terminal"], env=terminal_env,
                       check=False, capture_output=True)


if __name__ == "__main__":
    main()
