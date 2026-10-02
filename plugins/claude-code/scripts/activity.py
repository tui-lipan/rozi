#!/usr/bin/env python3
"""Translate Claude Code hooks into Rozi's public, sequence-fenced agent reports."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import sqlite3
import subprocess
import sys
import uuid


EVENT_STATES = {
    "UserPromptSubmit": ("working", None),
    "PostToolUse": ("working", None),
    "PostToolUseFailure": ("working", None),
    "PreCompact": ("working", None),
    "PostCompact": ("working", None),
    "ElicitationResult": ("working", None),
    "PermissionRequest": ("blocked", "Permission required"),
    "Elicitation": ("blocked", "Input required"),
    "StopFailure": ("idle", "Turn ended with an API error"),
    "SessionEnd": ("release", None),
}
NOTIFICATIONS = {
    "permission_prompt": ("blocked", "Permission required"),
    "elicitation_dialog": ("blocked", "Input required"),
    "elicitation_url_dialog": ("blocked", "Browser input required"),
    "elicitation_complete": ("working", None),
    "elicitation_response": ("working", None),
    "quota_auto_resume_fired": ("working", None),
    "quota_auto_resume_stale": ("blocked", "Usage limit reset; confirmation required"),
    "quota_auto_resume_disabled": ("idle", "Usage limit wait ended"),
}
CLI_TIMEOUT = 0.75


def session_start_transition(event: dict) -> tuple[str, str | None]:
    return ("working" if event.get("source") == "compact" else "idle"), None


def tool_transition(event: dict) -> tuple[str, str | None]:
    if event.get("tool_name") == "AskUserQuestion":
        return "blocked", "Question needs an answer"
    return "working", None


def stop_transition(event: dict) -> tuple[str, str | None]:
    if event.get("background_tasks") or event.get("session_crons"):
        return "working", "Background work remains"
    return "done", None


def notification_transition(event: dict) -> tuple[str, str | None] | None:
    return NOTIFICATIONS.get(event.get("notification_type"))


EVENT_HANDLERS = {
    "SessionStart": session_start_transition,
    "PreToolUse": tool_transition,
    "Stop": stop_transition,
    "Notification": notification_transition,
}


def transition(event: dict) -> tuple[str, str | None] | None:
    """Ignore subagent-local events and notifications about hidden sessions."""
    if event.get("agent_id") or event.get("agent_transcript_path"):
        return None
    name = event.get("hook_event_name")
    handler = EVENT_HANDLERS.get(name)
    if handler is not None:
        return handler(event)
    return EVENT_STATES.get(name)


def state_path(env: dict[str, str], parent_pid: int) -> Path:
    # Exec-form hooks run directly under Claude. A conversation id alone is not a
    # process identity: two processes can resume the same conversation.
    identity = [env["ROZI_SOCKET"], env["ROZI_SESSION_INSTANCE"], env["ROZI_PANE"], parent_pid]
    key = hashlib.sha256(json.dumps(identity).encode()).hexdigest()
    # Claude owns this persistent directory, separately from its cached plugin code.
    root = Path(env["CLAUDE_PLUGIN_DATA"]) / "activity"
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    return root / f"{key}.sqlite3"


def open_state(path: Path) -> sqlite3.Connection:
    # Exclusive creation keeps the database owner-only without changing the
    # process umask, including when hooks run concurrently.
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
    except FileExistsError:
        pass
    db = sqlite3.connect(path, timeout=0.2, isolation_level=None)
    db.execute("""CREATE TABLE IF NOT EXISTS run (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        token TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
        released INTEGER NOT NULL
    )""")
    return db


def start_run(db: sqlite3.Connection, row: tuple | None, event: dict, env: dict[str, str]) -> tuple:
    if event.get("source") == "compact" and row is not None and not row[3]:
        return row
    if row is not None and not row[3]:
        # Release the previous conversation before /resume claims this pane.
        # A new SessionStart also handles reuse of a parent PID after a crash.
        send_report(env, row, event, ("release", None))
    row = ("claude:" + uuid.uuid4().hex, event["session_id"], 0, 0)
    db.execute("INSERT OR REPLACE INTO run VALUES (1, ?, ?, ?, ?)", row)
    return row


def current_run(db: sqlite3.Connection, event: dict, env: dict[str, str]) -> tuple | None:
    session = event["session_id"]
    row = db.execute("SELECT token, session, seq, released FROM run").fetchone()
    if event["hook_event_name"] == "SessionStart":
        row = start_run(db, row, event, env)
    if row is None or row[3] or row[1] != session:
        return None
    return row


def send_report(env: dict[str, str], row: tuple, event: dict, change: tuple) -> None:
    state, reason = change
    args = [env.get("ROZI_BIN") or "rozi", "agents"]
    if state == "release":
        args += ["release"]
    else:
        args += ["report", "--agent", "claude", "--state", state,
                 "--native-session", event["session_id"]]
    args += ["--integration", row[0], "--seq", str(row[2] + 1)]
    if reason:
        args += ["--reason", reason]
    # ROZI_PANE selects the calling pane through ROZI_SOCKET. Never infer a
    # session name from a pane number, or open the platform-specific socket here.
    subprocess.run(args, env=env, stdin=subprocess.DEVNULL,
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                   timeout=CLI_TIMEOUT, check=False)


def handle(event: dict, env: dict[str, str], parent_pid: int) -> None:
    required = ("ROZI_PANE", "ROZI_SOCKET", "ROZI_SESSION_INSTANCE", "CLAUDE_PLUGIN_DATA")
    if env.get("ROZI") != "1" or not all(env.get(key) for key in required):
        return
    if not isinstance(event.get("session_id"), str) or not event["session_id"]:
        return
    change = transition(event)
    if change is None:
        return
    db = open_state(state_path(env, parent_pid))
    try:
        # Serialize sequence allocation and delivery. Async hooks would permit
        # old tool events to overwrite a newer permission or Stop event.
        db.execute("BEGIN IMMEDIATE")
        row = current_run(db, event, env)
        if row is not None:
            db.execute("UPDATE run SET seq = ?, released = ?",
                       (row[2] + 1, int(change[0] == "release")))
        try:
            if row is not None:
                send_report(env, row, event, change)
        finally:
            # Persist the sequence even on timeout: the UI may have received it.
            # Keep the transaction locked until delivery has finished.
            db.commit()
    finally:
        db.close()


def main() -> None:
    try:
        event = json.load(sys.stdin)
        if isinstance(event, dict):
            handle(event, dict(os.environ), os.getppid())
    except (OSError, ValueError, TypeError, sqlite3.Error, subprocess.SubprocessError):
        # An unavailable UI must never fail a tool call, veto Stop, or decide
        # whether Claude is permitted to run something. All output stays empty.
        pass


if __name__ == "__main__":
    main()
