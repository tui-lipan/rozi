#!/usr/bin/env python3
"""Capture every agent pane at once and say what detection currently reads each screen as.

The reading beside each screen is the whole point. A capture with no reading says nothing about
whether detection works; a capture that disagrees with what you set the pane to is the finding.

    capture.py                          # every pane with an agent behind it
    capture.py --target 12 15           # only these panes
    capture.py --expect working         # flag every pane that does not read this
    capture.py --watch 60               # sample for 60s, keep each state a pane passes through

Screens are scrubbed - home directory to `~`, hostname, any address-shaped text - and written to
target/agent-screens/. They are candidates, not fixtures: read one before it goes in the corpus.
Detection runs in the session server, so a rule change needs a server restart before it shows here.

Requires a running Rozi. Uses $ROZI_SOCKET, or --socket PATH.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import socket as socketlib
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
OUT = REPO / "target/agent-screens"

HOME = str(Path.home())
HOSTNAME = socketlib.gethostname()
USER = os.environ.get("USER") or os.environ.get("USERNAME") or ""
EMAIL = re.compile(r"[\w.+-]+@[\w-]+\.[\w.]+")


def rozi() -> str:
    local = REPO / "target/debug/rozi"
    return str(local) if local.exists() else "rozi"


def control(socket: str | None, *args: str) -> dict:
    env = dict(os.environ)
    if socket:
        env["ROZI_SOCKET"] = socket
    done = subprocess.run(
        [rozi(), *args], capture_output=True, text=True, env=env, check=False
    )
    if not done.stdout.strip():
        raise SystemExit(
            f"no reply from rozi {' '.join(args)}\n"
            f"{done.stderr.strip() or 'is a session running, and is ROZI_SOCKET set?'}"
        )
    reply = json.loads(done.stdout)
    if not reply.get("ok", False):
        raise SystemExit(f"rozi {' '.join(args)} failed: {reply.get('error', reply)}")
    return reply


def scrub(text: str) -> str:
    """Remove what a captured terminal picks up about the machine it ran on.

    Rules match substrings of already-lowercased text, so none of these substitutions change what a
    fixture reads - which is what makes it safe to do them before a person ever sees the file.
    """
    text = text.replace(HOME, "~")
    text = text.replace(HOSTNAME, "host")
    if USER:
        text = re.sub(rf"\b{re.escape(USER)}\b", "user", text)
    text = EMAIL.sub("user@example.com", text)
    return "\n".join(line.rstrip() for line in text.splitlines())


def agent_panes(socket: str | None, targets: list[str] | None) -> list[dict]:
    panes = control(socket, "list-panes")["data"]
    if targets:
        wanted = set(targets)
        return [p for p in panes if str(p["id"]) in wanted]
    return [p for p in panes if p.get("agent")]


def grab(socket: str | None, pane: dict) -> dict:
    cap = control(socket, "capture-pane", "--target", str(pane["id"]))["data"]
    return {
        "pane": pane["id"],
        "agent": pane.get("agent"),
        "reads_as": pane.get("agent_state"),
        "title": scrub(cap.get("title") or ""),
        "text": scrub(cap.get("text") or ""),
    }


def footer(text: str, rows: int = 8) -> list[str]:
    """The last `rows` non-empty lines - the region a `scope = "footer"` rule sees."""
    return [line for line in text.splitlines() if line.strip()][-rows:]


def describe(shot: dict) -> str:
    """One line per pane: what it reads as, and where its live signal is.

    The distance is what caught Cline. Its spinner rides the line being written and drifts up the
    screen, so it left the footer window mid-answer and a whole streaming turn read as idle. A
    signal more than eight non-empty lines from the bottom cannot be seen by a footer-scoped rule.
    """
    lines = [line for line in shot["text"].splitlines() if line.strip()]
    spins = [
        len(lines) - index
        for index, line in enumerate(lines)
        if line.lstrip() and 0x2800 <= ord(line.lstrip()[0]) <= 0x28FF
    ]
    title_spin = bool(shot["title"]) and 0x2800 <= ord(shot["title"][0]) <= 0x28FF
    marks = []
    if spins:
        near = min(spins)
        marks.append(f"spinner {near} from bottom" + ("" if near <= 8 else "  << OUT OF FOOTER"))
    if title_spin:
        marks.append("title spinner")
    if not marks:
        marks.append("no spinner")
    return f"  pane {shot['pane']:<4} {shot['agent'] or '-':<16} {shot['reads_as'] or '-':<8} {', '.join(marks)}"


def write(shot: dict, tag: str) -> Path:
    OUT.mkdir(parents=True, exist_ok=True)
    path = OUT / f"{shot['agent'] or 'pane'}-{shot['reads_as'] or 'unknown'}-{tag}.toml"
    body = [
        "# Candidate screen, not a fixture. Read it, trim the transcript, and move the case into",
        f"# tests/fixtures/agents/{shot['agent']}.toml with a comment saying what it is evidence of.",
        f"# Detection read this screen as `{shot['reads_as']}` when it was taken.",
        "",
        "[[case]]",
        'name = "CHANGE-ME"',
        f'state = "{shot["reads_as"]}"',
        f"title = {json.dumps(shot['title'])}",
        "screen = '''",
        shot["text"],
        "'''",
        "",
    ]
    path.write_text("\n".join(body), encoding="utf-8")
    return path


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", help="control socket path (default $ROZI_SOCKET)")
    parser.add_argument("--target", nargs="*", help="pane ids (default: every agent pane)")
    parser.add_argument(
        "--expect",
        choices=["idle", "working", "blocked"],
        help="flag any pane that does not read this",
    )
    parser.add_argument(
        "--watch",
        type=float,
        metavar="SECONDS",
        help="sample for this long, keeping one screen per state each pane passes through",
    )
    parser.add_argument(
        "--interval", type=float, default=1.0, help="seconds between samples while watching"
    )
    args = parser.parse_args()

    if args.watch:
        seen: dict[tuple[int, str], dict] = {}
        deadline = time.monotonic() + args.watch
        sample = 0
        while time.monotonic() < deadline:
            for pane in agent_panes(args.socket, args.target):
                shot = grab(args.socket, pane)
                key = (shot["pane"], shot["reads_as"] or "unknown")
                if key not in seen:
                    seen[key] = shot
                    print(f"[{sample:3d}] new state" + describe(shot)[1:], flush=True)
            sample += 1
            time.sleep(args.interval)
        shots = list(seen.values())
        print(f"\n{len(shots)} distinct pane states over {args.watch:.0f}s")
    else:
        shots = [grab(args.socket, pane) for pane in agent_panes(args.socket, args.target)]
        if not shots:
            raise SystemExit("no agent panes - is anything running, and does rozi recognize it?")
        print("What detection reads right now:\n")
        for shot in shots:
            print(describe(shot))

    tag = time.strftime("%H%M%S")
    print()
    for shot in shots:
        path = write(shot, tag)
        print(f"  wrote {path.relative_to(REPO)}")

    if args.expect:
        wrong = [s for s in shots if s["reads_as"] != args.expect]
        print()
        if wrong:
            print(f"NOT reading `{args.expect}` - these are the findings:")
            for shot in wrong:
                print(f"  pane {shot['pane']} ({shot['agent']}) reads {shot['reads_as']}")
        else:
            print(f"every pane reads `{args.expect}`.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
