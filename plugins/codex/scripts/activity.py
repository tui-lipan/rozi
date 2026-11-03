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
    # The first sign a turn was admitted. Hooks can still stop one before this, at SessionStart and
    # at UserPromptSubmit, and then no later hook runs, so a state said there would never be replaced.
    "PreModelRequest": ("working", None),
    "PostToolUse": ("working", None),
    "PreCompact": ("working", None),
    "PostCompact": ("working", None),
    "PermissionRequest": ("blocked", "Permission required"),
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
# Not a Codex hook: what the turn watcher records once Codex has written that the turn ended.
TURN_ENDED = "rozi:TurnEnded"
WATCH_POLL = 0.1
# How often the watcher checks that nothing has replaced the turn it waits on.
WATCH_STATE_EVERY = 1.0
# Another Stop hook can run for as long as its own timeout, ten minutes by default.
WATCH_LIMIT = 3600.0
CONFIRM_ATTEMPTS = 10


def watchable(event: dict) -> bool:
    turn, transcript = event.get("turn_id"), event.get("transcript_path")
    return (isinstance(turn, str) and bool(turn) and isinstance(transcript, str)
            and os.path.isfile(transcript))


def stop_transition(event: dict) -> tuple[str, str | None]:
    """The turn may have ended, but another Stop hook can still be running, and can send it on.

    Codex writes that the turn ended to its transcript only once every Stop hook has returned, so a
    watcher confirms it from there. Without a transcript to watch, this is the last word.
    Codex also opens dialogs after that, such as its offer to switch models, so the pane follows its
    screen before it reads done either way.
    """
    return ("working|done", None) if watchable(event) else ("working|done|ended", None)


def session_start_transition(event: dict) -> tuple[str | None, str | None]:
    """A compaction continues a run. Anything else only starts a lifecycle, and says no state.

    Codex fires this at a thread's first prompt, so idle would end the run screen detection just
    saw start and count it as finished.
    """
    return ("working" if event.get("source") == "compact" else None), None


def prompt_transition(_event: dict) -> tuple[str | None, str | None]:
    """A prompt can still be rejected, so it can claim the pane but says no state."""
    return None, None


def model_response_transition(event: dict) -> tuple[str | None, str | None]:
    """A failed request says nothing about whether Codex will retry, and no hook says it won't.

    A retry's next request replaces this, and the screen shows the retry until then.
    """
    if event.get("status") == "failed":
        return "working|error", "Turn ended with an API error"
    return None, None


def tool_transition(event: dict) -> tuple[str, str | None]:
    if event.get("tool_name") in QUESTION_TOOLS:
        return "blocked", "Question needs an answer"
    return "working", None


EVENT_HANDLERS = {
    "SessionStart": session_start_transition,
    "UserPromptSubmit": prompt_transition,
    "PostModelRequest": model_response_transition,
    "PreToolUse": tool_transition,
    "Stop": stop_transition,
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


def state_path(env: dict[str, str]) -> Path:
    # One store per pane. Codex starts each hook through `$SHELL -lc` or `cmd.exe /C`, and a shell
    # that does not exec its last command leaves a fresh parent process for every hook, so no
    # process id identifies the client. Successive clients in a pane are told apart by their
    # lifecycle tokens instead: each one's first SessionStart releases the previous token.
    # A pane with no UI to name (remote, or in a session no window shows) has no ROZI_SOCKET.
    identity = [env.get("ROZI_SOCKET", ""), env["ROZI_SESSION_INSTANCE"], env["ROZI_PANE"]]
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
    # Questions, keyed by the tool call that asks them.
    db.execute("""CREATE TABLE IF NOT EXISTS questions (
        call TEXT PRIMARY KEY, reason TEXT NOT NULL
    )""")
    # Tool calls that started and have not completed.
    db.execute("""CREATE TABLE IF NOT EXISTS calls (call TEXT PRIMARY KEY)""")
    # Open approvals. `waits` is whether the approval waits for the calls listed for it in
    # `approval_calls`; one with no call to wait for stays open until the turn ends.
    db.execute("""CREATE TABLE IF NOT EXISTS approvals (
        id INTEGER PRIMARY KEY, reason TEXT NOT NULL, waits INTEGER NOT NULL
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS approval_calls (
        approval INTEGER NOT NULL, call TEXT NOT NULL, PRIMARY KEY (approval, call)
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS releases (
        token TEXT PRIMARY KEY, seq INTEGER NOT NULL
    )""")
    db.execute("""CREATE TABLE IF NOT EXISTS attempts (
        token TEXT PRIMARY KEY, seq INTEGER NOT NULL
    )""")
    # The turn a Stop said may have ended, until a watcher confirms it or another report replaces it.
    db.execute("""CREATE TABLE IF NOT EXISTS ending (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1), turn TEXT NOT NULL
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
    # The previous lifecycle's undelivered state is not this one's to report.
    db.execute("UPDATE status SET dirty = 0")
    db.execute("DELETE FROM ending")
    clear_waits(db)
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
    # A token that never reported owns nothing to give back.
    if row[2] > 0:
        db.execute("INSERT OR IGNORE INTO releases VALUES (?, ?)", (row[0], row[2]))


def clear_waits(db: sqlite3.Connection) -> None:
    for table in ("questions", "calls", "approvals", "approval_calls"):
        db.execute(f"DELETE FROM {table}")


def call_id(event: dict) -> str | None:
    value = event.get("tool_use_id")
    return str(value) if value else None


def approval_calls(db: sqlite3.Connection, event: dict) -> list[str]:
    """The tool calls whose completion may end an approval.

    Codex's PermissionRequest names no tool call, and one call can raise several: a command's own
    approval, then network access or an escalated subcommand while it runs. Whichever call it
    belongs to is among those running, unless that call fired no PreToolUse. `write_stdin` fires
    none, and writes to a command that may have outlived the turn that started it, so its approval
    names that command's exec call instead."""
    if event.get("tool_name") == "write_stdin":
        tool_input = event.get("tool_input")
        parent = tool_input.get("parent_call_id") if isinstance(tool_input, dict) else None
        parent = parent or event.get("parent_call_id")
        if parent:
            return [str(parent)]
    return [row[0] for row in db.execute("SELECT call FROM calls")]


def open_approval(db: sqlite3.Connection, event: dict, reason: str) -> None:
    """Record an approval as waiting on every call it may belong to. Waiting for all of them, and
    holding an approval with none until the turn ends, never reports work while an approval may
    still be open."""
    calls = approval_calls(db, event)
    approval = db.execute("INSERT INTO approvals (reason, waits) VALUES (?, ?)",
                          (reason, int(bool(calls)))).lastrowid
    db.executemany("INSERT INTO approval_calls VALUES (?, ?)",
                   [(approval, call) for call in calls])


def finish_call(db: sqlite3.Connection, event: dict) -> None:
    call = call_id(event)
    if call is None:
        return
    db.execute("DELETE FROM calls WHERE call = ?", (call,))
    db.execute("DELETE FROM questions WHERE call = ?", (call,))
    db.execute("DELETE FROM approval_calls WHERE call = ?", (call,))
    db.execute("""DELETE FROM approvals WHERE waits = 1 AND NOT EXISTS
        (SELECT 1 FROM approval_calls WHERE approval_calls.approval = approvals.id)""")


def start_call(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    call = call_id(event)
    if call is None:
        return
    db.execute("INSERT OR IGNORE INTO calls VALUES (?)", (call,))
    if change[0] == "blocked":
        db.execute("INSERT OR REPLACE INTO questions VALUES (?, ?)", (call, change[1]))


def update_blockers(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    name = event["hook_event_name"]
    if name in {"Stop", "Interrupt", "SessionEnd", "UserPromptSubmit"}:
        clear_waits(db)
    elif name == "PreToolUse":
        start_call(db, event, change)
    elif name == "PostToolUse":
        finish_call(db, event)
    elif name == "PermissionRequest":
        open_approval(db, event, change[1])


def open_wait(db: sqlite3.Connection) -> tuple | None:
    return db.execute("""SELECT reason FROM questions
        UNION ALL SELECT reason FROM approvals LIMIT 1""").fetchone()


def ending_turn(db: sqlite3.Connection) -> str | None:
    row = db.execute("SELECT turn FROM ending").fetchone()
    return row[0] if row else None


def record_event(db: sqlite3.Connection, event: dict, change: tuple) -> None:
    row = current_run(db, event)
    if row is None:
        return
    name = event["hook_event_name"]
    # A confirmation counts only while the Stop it confirms is still the latest word.
    if name == TURN_ENDED and ending_turn(db) != event.get("turn_id"):
        return
    update_blockers(db, event, change)
    if change[0] is None:
        return
    db.execute("DELETE FROM ending")
    if name == "Stop" and change[0] == "working|done":
        db.execute("INSERT INTO ending VALUES (1, ?)", (event["turn_id"],))
    blocker = open_wait(db)
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
        state, *fallback = state.split("|")
        args += ["report", "--agent", "codex", "--state", state,
                 "--native-session", row[1]]
        if fallback:
            args += ["--unless-quiet", fallback[0]]
        if fallback[1:] == ["ended"]:
            args += ["--ended"]
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


def nested(event: dict, env: dict[str, str]) -> bool:
    """Whether this hook belongs to a Codex started from another Codex thread's tool shell, which
    inherits that thread's id and the pane, but is not what the pane shows."""
    inherited = env.get("CODEX_THREAD_ID")
    return bool(inherited) and inherited != event["session_id"]


def handle(event: dict, env: dict[str, str]) -> None:
    required = ("ROZI_PANE", "ROZI_SESSION_INSTANCE", "PLUGIN_DATA")
    if env.get("ROZI") != "1" or not all(env.get(key) for key in required):
        return
    if not isinstance(event.get("session_id"), str) or not event["session_id"]:
        return
    if nested(event, env):
        return
    change = transition(event)
    if change is None:
        return
    path = state_path(env)
    watch_from = None
    if change[0] == "working|done":
        # Measured before this hook returns: Codex writes that the turn ended only after that.
        try:
            watch_from = os.path.getsize(event["transcript_path"])
        except OSError:
            change = ("working|done|ended", None)
    # State persistence has its own retry budget. Delivery gets only its separate, smaller
    # budget after the event has committed, and never extends a failed persistence attempt.
    db = persist_event(path, event, change)
    try:
        if watch_from is not None and not start_watcher(env, event, watch_from):
            # Nothing would confirm it, so this is the last word after all.
            db.close()
            db = persist_event(path, ended_event(event), ("working|done|ended", None))
        flush_pending(db, path, env)
    finally:
        db.close()


def ended_event(event: dict) -> dict:
    return {"hook_event_name": TURN_ENDED, "session_id": event["session_id"],
            "turn_id": event["turn_id"]}


def start_watcher(env: dict[str, str], event: dict, offset: int) -> bool:
    """Outlive this hook, which Codex waits on before it can end the turn."""
    args = [sys.executable, os.path.abspath(__file__), "--watch-turn", event["session_id"],
            event["turn_id"], event["transcript_path"], str(offset)]
    options = {"start_new_session": True}
    if sys.platform == "win32":
        options = {"creationflags": subprocess.DETACHED_PROCESS
                   | subprocess.CREATE_NEW_PROCESS_GROUP}
    try:
        subprocess.Popen(args, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                         stderr=subprocess.DEVNULL, close_fds=True, **options)
    except OSError:
        return False
    return True


def turn_outcome(transcript: Path, offset: int, turn: str) -> tuple[str | None, int]:
    """Whether Codex has written that `turn` completed or was aborted since `offset`, and the
    offset to read from next. A line still being written is left for the next read."""
    with open(transcript, "rb") as file:
        file.seek(offset)
        data = file.read()
    for line in data.splitlines(keepends=True):
        if not line.endswith(b"\n"):
            break
        offset += len(line)
        try:
            record = json.loads(line)
        except ValueError:
            continue
        payload = record.get("payload") if isinstance(record, dict) else None
        if (record.get("type") == "event_msg" and isinstance(payload, dict)
                and payload.get("turn_id") == turn
                and payload.get("type") in {"task_complete", "turn_aborted"}):
            return payload["type"], offset
    return None, offset


def still_ending(path: Path, turn: str) -> bool:
    try:
        db = connect_private(path, STATE_LOCK_TIMEOUT)
        try:
            return ending_turn(db) == turn
        finally:
            db.close()
    except sqlite3.OperationalError as error:
        return is_lock_error(error)


def confirm_turn_end(env: dict[str, str], session: str, turn: str) -> None:
    path = state_path(env)
    event = ended_event({"session_id": session, "turn_id": turn})
    db = persist_event(path, event, ("working|done|ended", None))
    try:
        # No later hook may come to deliver what one attempt leaves behind.
        for _ in range(CONFIRM_ATTEMPTS):
            flush_pending(db, path, env)
            if db.execute("SELECT 1 FROM status WHERE dirty = 1").fetchone() is None:
                return
            time.sleep(WATCH_STATE_EVERY)
    finally:
        db.close()


def watch_turn(env: dict[str, str], session: str, turn: str, transcript: Path,
               offset: int) -> None:
    """Confirm `turn` ended once Codex has written so, unless another report replaced its Stop."""
    path = state_path(env)
    deadline = time.monotonic() + WATCH_LIMIT
    checked = time.monotonic()
    while time.monotonic() < deadline:
        outcome, offset = turn_outcome(transcript, offset, turn)
        if outcome == "task_complete":
            confirm_turn_end(env, session, turn)
            return
        # An interrupted turn is Interrupt's to report.
        if outcome is not None:
            return
        if time.monotonic() - checked >= WATCH_STATE_EVERY:
            if not still_ending(path, turn):
                return
            checked = time.monotonic()
        time.sleep(WATCH_POLL)


def main() -> None:
    try:
        if sys.argv[1:2] == ["--watch-turn"]:
            session, turn, transcript, offset = sys.argv[2:6]
            watch_turn(dict(os.environ), session, turn, Path(transcript), int(offset))
            return
        event = json.load(sys.stdin)
        if isinstance(event, dict):
            handle(event, dict(os.environ))
    except (OSError, ValueError, TypeError, sqlite3.Error, subprocess.SubprocessError):
        # An unavailable UI must never fail a tool call, veto Stop, or decide
        # whether Codex is permitted to run something. All output stays empty.
        pass


if __name__ == "__main__":
    main()
