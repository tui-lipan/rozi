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


def current_focus() -> tuple[str, str] | None:
    """The session and focused pane on screen; pane IDs alone are not unique."""
    result = subprocess.run(
        [ROZI, "layout", "get", "--format", "json"], capture_output=True, text=True, check=False
    )
    try:
        layout = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    client = layout.get("client") or {}
    pane = client.get("focused_pane")
    return (layout["session"], str(pane)) if pane is not None else None


def focused_pane(focus: tuple[str, str] | None) -> dict[str, object] | None:
    if focus is None:
        return None
    result = subprocess.run(
        [ROZI, "list-panes", "--format", "json"], capture_output=True, text=True, check=False
    )
    try:
        panes = json.loads(result.stdout)
    except json.JSONDecodeError:
        return None
    for entry in panes if isinstance(panes, list) else []:
        if (entry.get("session"), str(entry.get("id"))) == focus:
            return entry
    return None


class Publisher:
    """Keep one publisher bound to the focused pane, withdrawing it when ownership changes."""

    def __init__(self) -> None:
        self.owner: tuple[str, str, str] | None = None
        self.process: subprocess.Popen[str] | None = None

    def close(self) -> None:
        process = self.process
        self.process = None
        self.owner = None
        if process is None:
            return
        assert process.stdin is not None
        try:
            process.stdin.close()
        except BrokenPipeError:
            pass
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()

    def follow(self, pane: dict[str, object] | None) -> None:
        if pane is None:
            self.close()
            return
        reference = pane.get("reference") or {}
        instance = str(reference.get("session_instance", ""))
        owner = (str(pane["session"]), str(pane["id"]), instance)
        if owner != self.owner:
            self.close()
        if self.process is None:
            # Pin both namespaces. In particular, do not inherit the service's launch session.
            environment = dict(os.environ, ROZI_PANE=owner[1], ROZI_SESSION_INSTANCE=instance)
            self.process = subprocess.Popen(
                [ROZI, "publish"], stdin=subprocess.PIPE, text=True, env=environment
            )
            self.owner = owner

    def publish(self, pane: dict[str, object] | None, rows: dict[str, object]) -> bool:
        self.follow(pane)
        if self.process is None:
            return True
        if self.process.poll() is not None:
            return False
        assert self.process.stdin is not None
        try:
            self.process.stdin.write(json.dumps(rows) + "\n")
            self.process.stdin.flush()
        except BrokenPipeError:
            return False
        return True


def run() -> int:
    if os.environ.get("ROZI_EXTENSION") != "project-status":
        print("project-status must be started by Rozi", file=sys.stderr)
        return 2
    memory = DirtySince(Path(os.environ["ROZI_EXTENSION_STATE_DIR"]))
    publisher = Publisher()
    events = subprocess.Popen(
        [ROZI, "subscribe", "focus-changed", "pane-exited"], stdout=subprocess.PIPE, text=True
    )
    assert events.stdout is not None
    refresh = threading.Event()

    def follow_events() -> None:
        for line in events.stdout:
            event = json.loads(line)
            if event.get("event") in {"focus-changed", "pane-exited"}:
                refresh.set()

    threading.Thread(target=follow_events, daemon=True).start()
    try:
        while events.poll() is None:
            refresh.clear()
            now = time.time()
            # Poll authoritative focus even when a transition predates subscription readiness.
            pane = focused_pane(current_focus())
            directory = pane.get("cwd") if pane else None
            status = git_status(directory) if directory else None
            since = memory.update(status, now) if status else None
            if not publisher.publish(pane, snapshot(status, since, now)):
                # A pane can exit while the stream opens; let on-failure supervision retry.
                return 1
            refresh.wait(5)
    finally:
        publisher.close()
        events.terminate()
        try:
            events.wait(timeout=2)
        except subprocess.TimeoutExpired:
            events.kill()
            events.wait()
    # Rozi closes the streams when it reloads or disables the extension; exit rather than retry.
    return 0


if __name__ == "__main__":
    raise SystemExit(run())
