#!/usr/bin/env python3
"""Translate Claude Code hooks into Rozi's public, sequence-fenced agent reports."""

from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import random
import sqlite3
import subprocess
import sys
import time
import uuid


EVENT_STATES = {
    "UserPromptSubmit": ("working", None),
    "PostToolUse": ("working", None),
    "PostToolUseFailure": ("working", None),
    "PostToolBatch": ("working", None),
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
STATE_LOCK_TIMEOUT = 0.05
STATE_RETRY_BUDGET = 3.0
DELIVERY_LOCK_TIMEOUT = 0.05
DELIVERY_BUDGET = 0.9


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
    # A pane with no UI to name (remote, or in a session no window shows) has no ROZI_SOCKET.
    identity = [env.get("ROZI_SOCKET", ""), env["ROZI_SESSION_INSTANCE"], env["ROZI_PANE"], parent_pid]
    key = hashlib.sha256(json.dumps(identity).encode()).hexdigest()
    # Claude owns this persistent directory, separately from its cached plugin code.
    root = Path(env["CLAUDE_PLUGIN_DATA"]) / "activity"
    root.mkdir(mode=0o700, parents=True, exist_ok=True)
    return root / f"{key}.sqlite3"


def connect_private(path: Path, timeout: float) -> sqlite3.Connection:
    # Exclusive creation keeps the database owner-only without changing the
    # process umask, including when hooks run concurrently.
    try:
        fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        os.close(fd)
    except FileExistsError:
        pass
    return sqlite3.connect(path, timeout=timeout, isolation_level=None)


def initialize_state(db: sqlite3.Connection) -> None:
    db.execute("""CREATE TABLE IF NOT EXISTS run (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        token TEXT NOT NULL, session TEXT NOT NULL, seq INTEGER NOT NULL,
        released INTEGER NOT NULL
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS status (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        state TEXT NOT NULL, reason TEXT, dirty INTEGER NOT NULL
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS blockers (
        kind TEXT NOT NULL, identity TEXT NOT NULL, reason TEXT NOT NULL,
        count INTEGER NOT NULL DEFAULT 1,
        PRIMARY KEY (kind, identity)
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS releases (
        token TEXT PRIMARY KEY, seq INTEGER NOT NULL
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS attempts (
        token TEXT PRIMARY KEY, seq INTEGER NOT NULL
    )""")


def open_state(path: Path) -> sqlite3.Connection:
    db = connect_private(path, STATE_LOCK_TIMEOUT)
    try:
        with db:
            db.execute("BEGIN IMMEDIATE")
            initialize_state(db)
        return db
    except BaseException:
        db.close()
        raise


def is_lock_error(error: sqlite3.OperationalError) -> bool:
    return "locked" in str(error) or "busy" in str(error)


def persist_once(path: Path, event: dict, change: tuple) -> sqlite3.Connection:
    db = open_state(path)
    try:
        with db:
            db.execute("BEGIN IMMEDIATE")
            record_event(db, event, change)
        return db
    except BaseException:
        db.close()
        raise


def retry_delay(deadline: float) -> bool:
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        return False
    # Independent hook processes must not all retry ownership at the same instant.
    time.sleep(min(random.uniform(0.01, 0.05), remaining))
    return time.monotonic() < deadline


def persist_event(path: Path, event: dict, change: tuple) -> sqlite3.Connection:
    deadline = time.monotonic() + STATE_RETRY_BUDGET
    while True:
        try:
            return persist_once(path, event, change)
        except sqlite3.OperationalError as error:
            if not is_lock_error(error) or not retry_delay(deadline):
                raise


def start_run(db: sqlite3.Connection, row: tuple | None, event: dict) -> tuple:
    if event.get("source") == "compact" and row is not None and not row[3] and row[1] == event["session_id"]:
        return row
    if row is not None and not row[3]:
        queue_release(db, row)
    row = ("claude:" + uuid.uuid4().hex, event["session_id"], 0, 0)
    db.execute("INSERT OR REPLACE INTO run VALUES (1, ?, ?, ?, ?)", row)
    db.execute("DELETE FROM blockers")
    return row


def current_run(db: sqlite3.Connection, event: dict) -> tuple | None:
    session = event["session_id"]
    row = db.execute("SELECT token, session, seq, released FROM run").fetchone()
    if event["hook_event_name"] == "SessionStart":
        row = start_run(db, row, event)
    if row is None or row[3] or row[1] != session:
        return None
    return row


def queue_release(db: sqlite3.Connection, row: tuple) -> None:
    db.execute("INSERT OR IGNORE INTO releases VALUES (?, ?)", (row[0], row[2]))


def blocker_identity(event: dict) -> str:
    return str(event.get("tool_use_id") or "")


def elicitation_identity(event: dict) -> str:
    return json.dumps([event.get("mcp_server_name"), event.get("elicitation_id")])


def add_blocker(db: sqlite3.Connection, kind: str, identity: str, reason: str) -> None:
    db.execute("INSERT OR REPLACE INTO blockers (kind, identity, reason) VALUES (?, ?, ?)",
               (kind, identity, reason))


def finish_tool(db: sqlite3.Connection, event: dict) -> None:
    # PermissionRequest normally has no tool_use_id. An anonymous permission wait cannot be
    # cleared by any individual completion, including a parallel call to the same tool.
    identity = blocker_identity(event)
    if identity:
        db.execute("DELETE FROM blockers WHERE kind IN ('permission', 'question') AND identity = ?",
                   (identity,))


def start_mcp_wait(db: sqlite3.Connection, event: dict, reason: str) -> None:
    identity = elicitation_identity(event)
    if event.get("elicitation_id"):
        add_blocker(db, "elicitation", identity, reason)
        return
    # Form elicitations can omit their ID. Count requests from the same server so one response
    # cannot clear another outstanding request whose ID was also omitted.
    db.execute("""INSERT INTO blockers (kind, identity, reason) VALUES ('elicitation', ?, ?)
        ON CONFLICT (kind, identity) DO UPDATE SET count = count + 1""", (identity, reason))


def finish_mcp_wait(db: sqlite3.Connection, event: dict) -> None:
    identity = elicitation_identity(event)
    db.execute("UPDATE blockers SET count = count - 1 WHERE kind = 'elicitation' AND identity = ?",
               (identity,))
    db.execute("DELETE FROM blockers WHERE kind = 'elicitation' AND identity = ? AND count <= 0",
               (identity,))


def update_notification_blockers(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    kind = event.get("notification_type")
    if change[0] == "blocked":
        category = "quota" if kind == "quota_auto_resume_stale" else "notification"
        add_blocker(db, category, str(kind), change[1])
    elif kind in {"elicitation_complete", "elicitation_response"}:
        db.execute("DELETE FROM blockers WHERE kind = 'notification' AND identity LIKE 'elicitation_%'")
    else:
        db.execute("DELETE FROM blockers WHERE kind = 'quota'")


BLOCKER_STARTS = {
    "PermissionRequest": ("permission", blocker_identity),
    "PreToolUse": ("question", blocker_identity),
    "Elicitation": ("elicitation", elicitation_identity),
}


def start_blocker(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    if change[0] != "blocked":
        return
    if event["hook_event_name"] == "Elicitation":
        start_mcp_wait(db, event, change[1])
        return
    kind, identity = BLOCKER_STARTS[event["hook_event_name"]]
    add_blocker(db, kind, identity(event), change[1])


def update_blockers(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    name = event["hook_event_name"]
    if name in {"Stop", "StopFailure", "SessionEnd", "UserPromptSubmit"}:
        db.execute("DELETE FROM blockers")
    elif name == "PostToolBatch":
        db.execute("DELETE FROM blockers WHERE kind != 'quota'")
    elif name in {"PostToolUse", "PostToolUseFailure"}:
        finish_tool(db, event)
    elif name in BLOCKER_STARTS:
        start_blocker(db, event, change)
    elif name == "ElicitationResult":
        finish_mcp_wait(db, event)
    elif name == "Notification":
        update_notification_blockers(db, event, change)


def record_event(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    row = current_run(db, event)
    if row is None:
        return
    update_blockers(db, event, change)
    blocker = db.execute("SELECT reason FROM blockers ORDER BY kind, identity LIMIT 1").fetchone()
    state, reason = ("blocked", blocker[0]) if blocker else change
    released = state == "release"
    db.execute("UPDATE run SET seq = ?, released = ?", (row[2] + 1, int(released)))
    db.execute("INSERT OR REPLACE INTO status VALUES (1, ?, ?, ?)",
               (state, reason, int(not released)))
    if released:
        queue_release(db, row)


def send_report(env: dict[str, str], row: tuple, change: tuple, timeout: float) -> bool:
    state, reason = change
    args = [env.get("ROZI_BIN") or "rozi", "agents"]
    if state == "release":
        args += ["release"]
    else:
        args += ["report", "--agent", "claude", "--state", state,
                 "--native-session", row[1]]
    args += ["--integration", row[0], "--seq", str(row[2])]
    if reason:
        args += ["--reason", reason]
    # ROZI_PANE selects the calling pane: through ROZI_SOCKET when the pane has a UI, and
    # otherwise in the session server ROZI_SESSION_INSTANCE names, which rozi finds itself.
    # Never infer a session name from a pane number, or open the platform-specific socket here.
    result = subprocess.run(args, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE,
                            text=True, timeout=timeout, check=False)
    # A timed-out release may have reached the server. These two refusals establish that the
    # token no longer owns the pane; other failures must leave the release pending.
    already_released = state == "release" and result.stderr in {
        "integration token belongs to a retired agent incarnation\n",
        "integration is not the current owner of this pane\n",
    }
    return result.returncode == 0 or already_released


def next_delivery(db: sqlite3.Connection) -> tuple | None:
    with db:
        db.execute("BEGIN IMMEDIATE")
        release = db.execute("SELECT token, seq FROM releases ORDER BY rowid LIMIT 1").fetchone()
        if release:
            token, seq = release
            db.execute("UPDATE releases SET seq = ? WHERE token = ?", (seq + 1, token))
            return (token, "", seq + 1, "release", None)
        report = db.execute("""SELECT run.token, run.session, run.seq, status.state, status.reason
            FROM run JOIN status USING (singleton) WHERE status.dirty = 1""").fetchone()
        if report is None:
            return None
        token, session, seq, state, reason = report
        attempted = db.execute("SELECT seq FROM attempts WHERE token = ?", (token,)).fetchone()
        if attempted and attempted[0] >= seq:
            seq = attempted[0] + 1
            db.execute("UPDATE run SET seq = ? WHERE token = ?", (seq, token))
        db.execute("INSERT OR REPLACE INTO attempts VALUES (?, ?)", (token, seq))
        return token, session, seq, state, reason


def acknowledge_delivery(db: sqlite3.Connection, delivery: tuple) -> None:
    token, _, seq, state, _ = delivery
    with db:
        db.execute("BEGIN IMMEDIATE")
        if state == "release":
            db.execute("DELETE FROM releases WHERE token = ? AND seq = ?", (token, seq))
        else:
            db.execute("""UPDATE status SET dirty = 0 WHERE EXISTS
                (SELECT 1 FROM run WHERE token = ? AND seq = ?)""", (token, seq))


def acquire_sender(sender: sqlite3.Connection) -> bool:
    try:
        sender.execute("BEGIN IMMEDIATE")
        return True
    except sqlite3.OperationalError as error:
        if not is_lock_error(error):
            raise
        return False


def flush_pending(db: sqlite3.Connection, path: Path, env: dict[str, str]) -> None:
    # A separate database serializes senders without locking the lifecycle database. A hook
    # that cannot become sender leaves its committed event for this sender or the next hook.
    sender = connect_private(path.with_suffix(".delivery"), DELIVERY_LOCK_TIMEOUT)
    try:
        if not acquire_sender(sender):
            return
        deadline = time.monotonic() + DELIVERY_BUDGET
        while time.monotonic() < deadline:
            try:
                if not deliver_pending(db, env, deadline):
                    return
            except sqlite3.OperationalError as error:
                if not is_lock_error(error):
                    raise
                if not retry_delay(deadline):
                    return
    finally:
        sender.close()


def deliver_pending(db: sqlite3.Connection, env: dict[str, str], deadline: float) -> bool:
    delivery = next_delivery(db)
    if delivery is None:
        return False
    remaining = deadline - time.monotonic()
    if remaining <= 0:
        return False
    if not send_report(env, delivery[:3], delivery[3:], min(CLI_TIMEOUT, remaining)):
        return False
    acknowledge_delivery(db, delivery)
    return True


def handle(event: dict, env: dict[str, str], parent_pid: int) -> None:
    required = ("ROZI_PANE", "ROZI_SESSION_INSTANCE", "CLAUDE_PLUGIN_DATA")
    if env.get("ROZI") != "1" or not all(env.get(key) for key in required):
        return
    if not isinstance(event.get("session_id"), str) or not event["session_id"]:
        return
    change = transition(event)
    if change is None:
        return
    path = state_path(env, parent_pid)
    # State persistence has its own retry budget. Delivery gets only its separate, smaller
    # budget after the event has committed, and never extends a failed persistence attempt.
    db = persist_event(path, event, change)
    try:
        flush_pending(db, path, env)
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
