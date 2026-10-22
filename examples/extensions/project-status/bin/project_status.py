#!/usr/bin/env python3
"""Follow the focused pane's project and publish its Git status as one Activity row.

The service remembers when each project last became dirty, in the extension's state directory,
so "uncommitted for 3h" survives reloads, updates, and restarts.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from pathlib import Path

ROZI = os.environ.get("ROZI_BIN", "rozi")


@dataclass(frozen=True)
class Status:
    root: str
    branch: str
    changed: list[str]


def git_status(directory: str) -> Status | None:
    """The repository containing `directory`, or None when it is not in one."""
    def git(*arguments: str) -> str | None:
        result = subprocess.run(
            ["git", "-C", directory, *arguments], capture_output=True, text=True, check=False
        )
        return result.stdout if result.returncode == 0 else None

    root = git("rev-parse", "--show-toplevel")
    porcelain = git("status", "--porcelain=v1", "--branch")
    if root is None or porcelain is None:
        return None
    return parse_status(root.strip(), porcelain)


def parse_status(root: str, porcelain: str) -> Status:
    branch = "detached"
    changed = []
    for line in porcelain.splitlines():
        if line.startswith("## "):
            head = line[3:].split("...", 1)[0]
            branch = head.removeprefix("No commits yet on ")
        elif line.strip():
            changed.append(line[3:])
    return Status(root, branch, changed)


class DirtySince:
    """When each project last went from clean to dirty, kept in the state directory."""

    def __init__(self, state_dir: Path) -> None:
        self.path = state_dir / "dirty-since.json"
        try:
            stored = json.loads(self.path.read_text())
        except (OSError, json.JSONDecodeError):
            stored = {}
        self.times: dict[str, float] = stored if isinstance(stored, dict) else {}

    def update(self, status: Status, now: float) -> float | None:
        before = dict(self.times)
        if status.changed:
            self.times.setdefault(status.root, now)
        else:
            self.times.pop(status.root, None)
        if self.times != before:
            self.save()
        return self.times.get(status.root)

    def save(self) -> None:
        # Several services may share the directory; write whole files and rename them into place.
        temporary = self.path.with_suffix(f".{os.getpid()}.tmp")
        temporary.write_text(json.dumps(self.times, indent=2, sort_keys=True))
        temporary.replace(self.path)


def duration(seconds: float) -> str:
    minutes = int(seconds // 60)
    if minutes < 60:
        return f"{minutes}m"
    hours = minutes // 60
    return f"{hours}h" if hours < 48 else f"{hours // 24}d"


def snapshot(status: Status | None, since: float | None, now: float) -> dict[str, object]:
    """What `rozi publish` reads: no row outside a repository, else one for the project."""
    if status is None:
        return {"rows": []}
    count = len(status.changed)
    reason = "clean" if count == 0 else f"{count} uncommitted file{'s' if count != 1 else ''}"
    if count and since is not None:
        reason += f" for {duration(now - since)}"
    row = {
        "id": "project",
        "title": f"{Path(status.root).name} · {status.branch}",
        "status": "working" if count else "idle",
        "reason": reason,
    }
    return {"rows": [row]}


def current_focus() -> str | None:
    """The UI's focused pane, which list-panes does not identify."""
    result = subprocess.run(
        [ROZI, "layout", "get", "--format", "json"], capture_output=True, text=True, check=False
    )
    try:
        layout = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    client = layout.get("client") or {}
    pane = client.get("focused_pane")
    return str(pane) if pane is not None else None


def focused_cwd(pane: str | None) -> str | None:
    if pane is None:
        return None
    result = subprocess.run(
        [ROZI, "list-panes", "--format", "json"], capture_output=True, text=True, check=False
    )
    try:
        panes = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    for entry in panes if isinstance(panes, list) else []:
        if str(entry.get("id")) == pane:
            return entry.get("cwd")
    return None


def run() -> int:
    if os.environ.get("ROZI_EXTENSION") != "project-status":
        print("project-status must be started by Rozi", file=sys.stderr)
        return 2
    memory = DirtySince(Path(os.environ["ROZI_EXTENSION_STATE_DIR"]))
    publisher = subprocess.Popen([ROZI, "publish"], stdin=subprocess.PIPE, text=True)
    events = subprocess.Popen(
        [ROZI, "subscribe", "focus-changed", "pane-exited"], stdout=subprocess.PIPE, text=True
    )
    assert publisher.stdin is not None and events.stdout is not None
    refresh = threading.Event()

    def follow_events() -> None:
        for line in events.stdout:
            event = json.loads(line)
            if event.get("event") in {"focus-changed", "pane-exited"}:
                refresh.set()

    threading.Thread(target=follow_events, daemon=True).start()
    while publisher.poll() is None and events.poll() is None:
        refresh.clear()
        now = time.time()
        # Popen cannot acknowledge subscription readiness. Poll authoritative focus even
        # without an event, so a transition before registration cannot leave us stale.
        directory = focused_cwd(current_focus())
        status = git_status(directory) if directory else None
        since = memory.update(status, now) if status else None
        try:
            publisher.stdin.write(json.dumps(snapshot(status, since, now)) + "\n")
            publisher.stdin.flush()
        except BrokenPipeError:
            break
        refresh.wait(5)
    # Rozi closes the streams when it reloads or disables the extension; exit rather than retry.
    return 0


if __name__ == "__main__":
    raise SystemExit(run())
