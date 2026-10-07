#!/usr/bin/env python3
"""Linux-only CPU probe for owned, isolated rozi clients and session servers.

Build the selected binary first. Run before/after serially, without Cargo or other
CPU-intensive work. Output is JSON lines; all PTYs and user directories are private.
"""

import argparse
import json
import math
import os
from pathlib import Path
import shutil
import shlex
import signal
import subprocess
import sys
import tempfile
import time


WORKLOAD = r'''
import os, pathlib, sys, time
root = pathlib.Path(__file__).parent
while not (root / "start").exists(): time.sleep(.05)
sys.stdout.write("\x1b[?1049h\x1b[?25l")
for row in range(1, 50): sys.stdout.write(f"\x1b[{row};1Hbody content row {row}")
sys.stdout.write("\x1b[1;1HREADY"); sys.stdout.flush()
tick = 0
while True:
    mode = (root / "mode").read_text().strip()
    spinner = "|/"[tick % 2]
    if mode in ("body", "both"):
        sys.stdout.write(f"\x1b[50;1H{spinner} working\x1b[K")
    if mode in ("title", "both"):
        sys.stdout.write(f"\x1b]2;{spinner} Investigate CPU | project\x07")
    if mode == "dense":
        for row in range(1, os.get_terminal_size().lines + 1):
            marker = "READY " if row == 1 else ""
            sys.stdout.write(f"\x1b[{row};1H{marker}{spinner} changing row {row}\x1b[K")
    sys.stdout.flush(); tick += 1; time.sleep(.1)
'''


def cpu_ticks(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def await_value(read, description):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        value = read()
        if value:
            return value
        time.sleep(.05)
    raise RuntimeError(f"timed out waiting for {description}")


def run_probe(args):
    binary = str(Path(args.binary).resolve(strict=True))
    root = Path(tempfile.mkdtemp(prefix="rozi-cpu-"))
    env = {key: value for key, value in os.environ.items() if not key.startswith("ROZI")}
    for key, directory in (
        ("HOME", "home"), ("XDG_CONFIG_HOME", "config"),
        ("XDG_STATE_HOME", "state"), ("XDG_CACHE_HOME", "cache"),
        ("XDG_DATA_HOME", "data"), ("XDG_RUNTIME_DIR", "runtime"),
    ):
        path = root / directory
        path.mkdir()
        env[key] = str(path)
    env.update(ROZI_CONFIG=str(root / "config/config.toml"), TERM="xterm-256color", SHELL="/bin/sh")
    (root / "work.py").write_text(WORKLOAD)
    (root / "mode").write_text("idle")
    # JSON strings are valid TOML basic strings for these paths.
    (root / "config/config.toml").write_text(f'''
shell = [{json.dumps(sys.executable)}, {json.dumps(str(root / "work.py"))}]
command_shell = ["/bin/sh", "-c"]
cwd = {json.dumps(str(root))}
[shell_integration]
mode = "off"
[session]
autosave = false
resurrect = false
[confirm]
kill_session = false
[animations]
enabled = false
''')
    server = wrapper = None
    client_pid = None
    session = "cpu-probe"

    def command(*argv):
        return subprocess.check_output([binary, *argv], env=env, text=True)

    def stop_client():
        nonlocal wrapper, client_pid
        if client_pid is not None:
            try:
                os.kill(client_pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            client_pid = None
        if wrapper is not None:
            try:
                wrapper.wait(timeout=10)
            except subprocess.TimeoutExpired:
                wrapper.terminate()
                wrapper.wait(timeout=10)
            wrapper = None

    try:
        print(json.dumps({"binary": binary, "version": command("--version").strip(),
                          "sample_seconds": args.seconds, "updates_per_second": 10}), flush=True)
        with (root / "server.log").open("wb") as log:
            server = subprocess.Popen([binary, "--session", session, "--fresh-server"],
                                      env=env, stdout=log, stderr=log)
        await_value(lambda: list((root / "runtime/rozi").glob("session-*.sock")), "server socket")
        for cols, rows in [(200, 60), (475, 116)]:
            with (root / f"client-{cols}.log").open("wb") as log:
                # The binary path is shell quoted because script's command is interpreted by sh.
                launch = f"stty rows {rows} cols {cols}; exec {shlex.quote(binary)} sessions attach {session}"
                wrapper = subprocess.Popen(["script", "-qefc", launch, "/dev/null"], env=env,
                                           stdin=subprocess.PIPE, stdout=log, stderr=log)
            sockets = await_value(lambda: list((root / "runtime/rozi").glob("control-*.sock")), "client socket")
            socket = sockets[0]
            client_pid = int(socket.stem.removeprefix("control-"))
            # The UI's temporary startup pane can precede server attachment. Resolve the live id
            # at the server, then use its wait rather than fencing a temporary client generation.
            panes = await_value(lambda: json.loads(command("--session", session, "list-panes", "--format", "json"))["data"], "server pane")
            pane = str(panes[0]["id"])
            (root / "start").touch()
            command("--session", session, "capture-pane", "--target", pane,
                    "--wait-for", "READY", "--timeout", "15s")
            for mode in ["idle", "body", "both", "title", "body", "dense"]:
                (root / "next-mode").write_text(mode)
                (root / "next-mode").replace(root / "mode")
                time.sleep(1)
                before = [cpu_ticks(client_pid), cpu_ticks(server.pid)]
                start = time.monotonic()
                time.sleep(args.seconds)
                elapsed = time.monotonic() - start
                after = [cpu_ticks(client_pid), cpu_ticks(server.pid)]
                scale = 100 / os.sysconf("SC_CLK_TCK") / elapsed
                print(json.dumps({"cols": cols, "rows": rows, "mode": mode, "seconds": elapsed,
                                  "client_cpu": (after[0] - before[0]) * scale,
                                  "server_cpu": (after[1] - before[1]) * scale}), flush=True)
            if args.keep_scratch:
                # Capture after measurement, so PNG encoding cannot contaminate CPU samples.
                command("--socket", str(socket), "capture-ui", "--render", "png",
                        "--output", str(root / f"ui-{cols}.png"))
            stop_client()
            await_value(lambda: not socket.exists(), "client shutdown")
    finally:
        subprocess.run([binary, "sessions", "kill", session], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=15)
        stop_client()
        if server is not None and server.poll() is None:
            server.terminate()
            server.wait(timeout=10)
        if args.keep_scratch:
            print(json.dumps({"scratch": str(root)}), flush=True)
        else:
            shutil.rmtree(root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", help="already built release binary to measure")
    parser.add_argument("--seconds", type=float, default=15, help="sample window per case (default: 15)")
    parser.add_argument("--keep-scratch", action="store_true", help="retain synthetic logs and config")
    args = parser.parse_args()
    if sys.platform != "linux" or not shutil.which("script"):
        parser.error("requires Linux /proc and util-linux script")
    if not math.isfinite(args.seconds) or args.seconds <= 0:
        parser.error("--seconds must be positive")
    run_probe(args)


if __name__ == "__main__":
    main()
