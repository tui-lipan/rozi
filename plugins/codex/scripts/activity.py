#!/usr/bin/env python3
"""Translate Codex hooks into Rozi's public, sequence-fenced agent reports."""

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
    "PostToolUse": ("working", None),
    "PreCompact": ("working", None),
    "PostCompact": ("working", None),
    "PermissionRequest": ("blocked", "Permission required"),
    "Stop": ("done", None),
    "Interrupt": ("idle", None),
    "SessionEnd": ("release", None),
}
# Codex's tool for asking the user a question in the middle of a turn.
QUESTION_TOOLS = {"request_user_input"}
CLI_TIMEOUT = 0.75
STATE_LOCK_TIMEOUT = 0.05
STATE_RETRY_BUDGET = 3.0
DELIVERY_LOCK_TIMEOUT = 0.05
DELIVERY_BUDGET = 0.9
# Codex caps an Interrupt hook at three seconds, so its persistence retries stop sooner.
SHORT_HOOK_BUDGETS = {"Interrupt": 1.2}


def session_start_transition(event: dict) -> tuple[str, str | None]:
    return ("working" if event.get("source") == "compact" else "idle"), None


def prompt_transition(_event: dict) -> tuple[str, str | None]:
    return "working", None


def tool_transition(event: dict) -> tuple[str, str | None]:
    if event.get("tool_name") in QUESTION_TOOLS:
        return "blocked", "Question needs an answer"
    return "working", None


EVENT_HANDLERS = {
    "SessionStart": session_start_transition,
    "UserPromptSubmit": prompt_transition,
    "PreToolUse": tool_transition,
}


def transition(event: dict) -> tuple[str, str | None] | None:
    """Ignore sub-agent events, which Codex delivers under the parent's session."""
    if event.get("agent_id") or event.get("agent_transcript_path"):
        return None
    name = event.get("hook_event_name")
    handler = EVENT_HANDLERS.get(name)
    if handler is not None:
        return handler(event)
    return EVENT_STATES.get(name)


def state_path(env: dict[str, str], parent_pid: int) -> Path:
    # Codex runs hook commands as children of the TUI process, even when the thread itself runs
    # in its shared app-server daemon. A thread id alone is not a process identity: two TUIs can
    # open the same thread.
    identity = [env["ROZI_SOCKET"], env["ROZI_SESSION_INSTANCE"], env["ROZI_PANE"], parent_pid]
    key = hashlib.sha256(json.dumps(identity).encode()).hexdigest()
    # Codex owns this persistent directory, separately from its cached plugin code.
    root = Path(env["PLUGIN_DATA"]) / "activity"
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
    budget = SHORT_HOOK_BUDGETS.get(event["hook_event_name"], STATE_RETRY_BUDGET)
    deadline = time.monotonic() + budget
    while True:
        try:
            return persist_once(path, event, change)
        except sqlite3.OperationalError as error:
            if not is_lock_error(error) or not retry_delay(deadline):
                raise


def start_run(db: sqlite3.Connection, row: tuple | None, event: dict) -> tuple:
    if row is not None and not row[3]:
        queue_release(db, row)
    row = ("codex:" + uuid.uuid4().hex, event["session_id"], 0, 0)
    db.execute("INSERT OR REPLACE INTO run VALUES (1, ?, ?, ?, ?)", row)
    db.execute("DELETE FROM blockers")
    return row


def claims_pane(event: dict, row: tuple | None) -> bool:
    """Whether this event names the thread the TUI now shows, replacing the current lifecycle.

    Codex starts a thread's hooks lazily, at its first prompt in this TUI, so `/resume` reports
    nothing until then. A prompt is the one event only the thread on screen receives: switching
    back to a thread this TUI already started fires no second SessionStart.
    """
    name = event["hook_event_name"]
    if name == "SessionStart":
        compacting = event.get("source") == "compact"
        return not (compacting and row is not None and not row[3] and row[1] == event["session_id"])
    if name == "UserPromptSubmit":
        return row is None or row[1] != event["session_id"]
    return False


def current_run(db: sqlite3.Connection, event: dict) -> tuple | None:
    session = event["session_id"]
    row = db.execute("SELECT token, session, seq, released FROM run").fetchone()
    if claims_pane(event, row):
        row = start_run(db, row, event)
    if row is None or row[3] or row[1] != session:
        return None
    return row


def queue_release(db: sqlite3.Connection, row: tuple) -> None:
    db.execute("INSERT OR IGNORE INTO releases VALUES (?, ?)", (row[0], row[2]))


def call_identity(event: dict) -> str:
    """The tool call an event is about, from what both its permission request and its completion
    carry. Codex's PermissionRequest has no tool_use_id, and adds the approval prompt's own
    `description` to the tool input."""
    tool_input = event.get("tool_input")
    if event["hook_event_name"] == "PermissionRequest" and isinstance(tool_input, dict):
        tool_input = {key: value for key, value in tool_input.items() if key != "description"}
    call = [event.get("tool_name"), tool_input]
    return hashlib.sha256(json.dumps(call, sort_keys=True, default=str).encode()).hexdigest()


def start_permission_wait(db: sqlite3.Connection, event: dict, reason: str) -> None:
    # Identical parallel calls share an identity. Counting them keeps one completion from
    # clearing another call's outstanding approval.
    db.execute("""INSERT INTO blockers (kind, identity, reason) VALUES ('permission', ?, ?)
        ON CONFLICT (kind, identity) DO UPDATE SET count = count + 1""",
               (call_identity(event), reason))


def finish_tool(db: sqlite3.Connection, event: dict) -> None:
    identity = call_identity(event)
    db.execute("UPDATE blockers SET count = count - 1 WHERE kind = 'permission' AND identity = ?",
               (identity,))
    db.execute("DELETE FROM blockers WHERE kind = 'permission' AND identity = ? AND count <= 0",
               (identity,))
    if event.get("tool_use_id"):
        db.execute("DELETE FROM blockers WHERE kind = 'question' AND identity = ?",
                   (str(event["tool_use_id"]),))


def update_blockers(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    name = event["hook_event_name"]
    if name in {"Stop", "Interrupt", "SessionEnd", "UserPromptSubmit"}:
        db.execute("DELETE FROM blockers")
    elif name == "PostToolUse":
        finish_tool(db, event)
    elif name == "PermissionRequest":
        start_permission_wait(db, event, change[1])
    elif name == "PreToolUse" and change[0] == "blocked":
        db.execute("INSERT OR REPLACE INTO blockers (kind, identity, reason) VALUES ('question', ?, ?)",
                   (str(event.get("tool_use_id") or ""), change[1]))


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
        args += ["report", "--agent", "codex", "--state", state,
                 "--native-session", row[1]]
    args += ["--integration", row[0], "--seq", str(row[2])]
    if reason:
        args += ["--reason", reason]
    # ROZI_PANE selects the calling pane through ROZI_SOCKET. Never infer a
    # session name from a pane number, or open the platform-specific socket here.
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
    required = ("ROZI_PANE", "ROZI_SOCKET", "ROZI_SESSION_INSTANCE", "PLUGIN_DATA")
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
        # whether Codex is permitted to run something. All output stays empty.
        pass


if __name__ == "__main__":
    main()
