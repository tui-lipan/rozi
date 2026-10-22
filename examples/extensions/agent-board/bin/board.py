#!/usr/bin/env python3
"""List the session's agents in the Agents tab, and sum them up in one Activity row.

Rozi runs the board on the active session's host and restarts it when you switch sessions.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time

ROZI = os.environ.get("ROZI_BIN", "rozi")
TONES = {
    "working": "working",
    "blocked": "blocked",
    "idle": "muted",
    "done": "success",
    "error": "error",
}


def agent_panes(panes: list[dict[str, object]]) -> list[dict[str, object]]:
    return [pane for pane in panes if pane.get("agent")]


def state_of(pane: dict[str, object]) -> str:
    """list-panes already projects integration reports, pane reports, and detection."""
    return str(pane.get("agent_state") or "idle")


def tab_snapshot(agents: list[dict[str, object]]) -> dict[str, object]:
    """Rows for `rozi sidebar publish`: one per agent pane."""
    return {
        "rows": [
            {
                "id": str(pane["id"]),
                "label": str(pane.get("title") or pane["agent"]),
                "description": str(pane["agent"]),
                "meta": state_of(pane),
                "tone": TONES.get(state_of(pane), "normal"),
            }
            for pane in agents
        ]
    }


def activity_snapshot(agents: list[dict[str, object]]) -> dict[str, object]:
    """One Activity row for `rozi publish`, worst state first, or none without agents."""
    if not agents:
        return {"rows": []}
    states = [state_of(pane) for pane in agents]
    waiting = states.count("blocked")
    working = states.count("working")
    urgency = ("blocked", "error", "working", "done", "idle")
    status = next((state for state in urgency if state in states), "idle")
    reason = f"{waiting} waiting for you, {working} working, {len(agents)} in all"
    return {"rows": [{"id": "agents", "title": "Agents", "status": status, "reason": reason}]}


def list_panes() -> list[dict[str, object]]:
    result = subprocess.run(
        [ROZI, "list-panes", "--format", "json"], capture_output=True, text=True, check=False
    )
    try:
        panes = json.loads(result.stdout)
    except json.JSONDecodeError:
        return []
    return panes if isinstance(panes, list) else []


def handle(event: dict[str, object]) -> list[str] | None:
    """The `rozi` command a sidebar event asks for: activating a row focuses its pane."""
    pane = event.get("activate")
    return [ROZI, "focus", str(pane)] if isinstance(pane, str) else None


def run() -> int:
    if os.environ.get("ROZI_EXTENSION") != "agent-board":
        print("agent-board must be started by Rozi", file=sys.stderr)
        return 2
    tab = subprocess.Popen(
        [ROZI, "sidebar", "publish", "--tab", "agents"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    activity = subprocess.Popen([ROZI, "publish"], stdin=subprocess.PIPE, text=True)
    assert tab.stdin is not None and tab.stdout is not None and activity.stdin is not None

    def read_events() -> None:
        for line in tab.stdout:
            command = handle(json.loads(line))
            if command:
                subprocess.run(command, check=False)

    threading.Thread(target=read_events, daemon=True).start()
    while tab.poll() is None and activity.poll() is None:
        agents = agent_panes(list_panes())
        snapshots = [(tab, tab_snapshot(agents)), (activity, activity_snapshot(agents))]
        try:
            for stream, snapshot in snapshots:
                stream.stdin.write(json.dumps(snapshot) + "\n")
                stream.stdin.flush()
        except BrokenPipeError:
            break
        time.sleep(2)
    return 0


if __name__ == "__main__":
    raise SystemExit(run())
