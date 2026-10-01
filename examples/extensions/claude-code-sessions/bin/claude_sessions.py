#!/usr/bin/env python3
"""Publish every Claude Code background session as a row of the pane running Claude's session view.

Claude Code runs background sessions under its own daemon, each in its own terminal and often in
its own Git worktree, and shows them from one client. Rozi sees that client as one pane and its
screen as one agent. This service asks Claude for the sessions it runs and publishes one Activity
row per session into that pane, each with the directory the session works in, so the sidebar
groups it under its repository and branch.

Everything goes through public interfaces: `claude agents --json`, `rozi list-panes`, and one
`rozi publish` stream per pane.
"""

from __future__ import annotations

import json
import os
import queue
import signal
import subprocess
import sys
import threading
from dataclasses import dataclass
from typing import Any


ROZI = os.environ.get("ROZI_BIN", "rozi")
EXTENSION_ID = "claude-code-sessions"
COMMAND_TIMEOUT = 5.0
# Claude's own names for a session waiting on the user. Anything unrecognized falls back on its
# coarse busy/idle `status`, never on a custom word, which Rozi would read as a live run.
BLOCKED_STATES = {
    "blocked",
    "waiting",
    "needs_input",
    "needs-input",
    "awaiting_input",
    "input_required",
    "permission",
}
FAILED_STATES = {"failed", "error", "errored", "crashed"}


class SessionsError(RuntimeError):
    """A public CLI call failed or answered something unreadable."""


@dataclass(frozen=True)
class Settings:
    claude: str = "claude"
    poll_seconds: float = 2.0
    scope: str = "all"

    @classmethod
    def from_environment(cls, environ: dict[str, str]) -> Settings:
        try:
            values = json.loads(environ.get("ROZI_EXTENSION_CONFIG") or "{}")
        except json.JSONDecodeError:
            values = {}
        if not isinstance(values, dict):
            values = {}
        claude = values.get("claude")
        scope = values.get("scope")
        try:
            poll = float(values.get("poll_seconds", cls.poll_seconds))
        except (TypeError, ValueError):
            poll = cls.poll_seconds
        return cls(
            claude=claude if isinstance(claude, str) and claude.strip() else cls.claude,
            poll_seconds=min(max(poll, 0.5), 60.0),
            scope=scope if scope in {"all", "cwd"} else cls.scope,
        )


@dataclass(frozen=True)
class Session:
    session_id: str
    pid: int | None
    kind: str
    cwd: str | None
    name: str | None
    state: str
    status: str
    started_at: int

    @classmethod
    def from_wire(cls, value: object) -> Session | None:
        if not isinstance(value, dict):
            return None
        session_id = text(value.get("sessionId")) or text(value.get("id"))
        if session_id is None:
            return None
        return cls(
            session_id=session_id,
            pid=integer(value.get("pid")),
            kind=(text(value.get("kind")) or "").casefold(),
            cwd=text(value.get("cwd")),
            name=text(value.get("name")),
            state=(text(value.get("state")) or "").casefold(),
            status=(text(value.get("status")) or "").casefold(),
            started_at=integer(value.get("startedAt")) or 0,
        )

    def row_state(self) -> tuple[str, str | None]:
        """The Rozi status this session shows, and why when the word alone does not say."""
        if self.state in BLOCKED_STATES:
            return "blocked", None
        if self.state in FAILED_STATES:
            return "idle", "Session failed"
        if self.state == "stopped":
            return "idle", "Stopped"
        if self.state in {"working", "done", "idle"}:
            return self.state, None
        return ("working" if self.status == "busy" else "idle"), None

    def row(self) -> dict[str, object]:
        status, reason = self.row_state()
        row: dict[str, object] = {
            "id": self.session_id,
            "title": self.name or "",
            "status": status,
            # Claude does not say which session its client has on screen.
            "active": False,
        }
        if reason:
            row["reason"] = reason
        if self.cwd:
            row["cwd"] = self.cwd
        return row


@dataclass(frozen=True)
class Pane:
    pane_id: int
    cwd: str | None
    program: str | None
    agent: str | None
    foreground_pid: int | None

    @classmethod
    def from_wire(cls, value: object) -> Pane | None:
        if not isinstance(value, dict):
            return None
        pane_id = integer(value.get("id"))
        if pane_id is None:
            return None
        return cls(
            pane_id=pane_id,
            cwd=text(value.get("cwd")),
            program=text(value.get("foreground_program")),
            agent=text(value.get("agent")),
            foreground_pid=integer(value.get("foreground_pid")),
        )

    def runs_claude(self) -> bool:
        return self.agent == "claude" or (self.program or "").casefold() in {
            "claude",
            "claude-code",
        }


def text(value: object) -> str | None:
    if value is None:
        return None
    value = str(value).strip()
    return value or None


def integer(value: object) -> int | None:
    if isinstance(value, bool):
        return None
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return None


def parse_sessions(output: str) -> list[Session]:
    try:
        data = json.loads(output)
    except json.JSONDecodeError as error:
        raise SessionsError("claude agents --json returned invalid JSON") from error
    if not isinstance(data, list):
        raise SessionsError("claude agents --json returned a non-list payload")
    return [session for item in data if (session := Session.from_wire(item)) is not None]


def parse_panes(output: str) -> list[Pane]:
    try:
        response = json.loads(output)
    except json.JSONDecodeError as error:
        raise SessionsError("rozi list-panes returned invalid JSON") from error
    if not isinstance(response, dict) or response.get("ok") is not True:
        detail = response.get("error") if isinstance(response, dict) else None
        raise SessionsError(str(detail or "rozi list-panes failed"))
    data = response.get("data")
    if not isinstance(data, list):
        raise SessionsError("rozi list-panes returned a non-list payload")
    return [pane for item in data if (pane := Pane.from_wire(item)) is not None]


def host_panes(panes: list[Pane], sessions: list[Session]) -> list[Pane]:
    """Panes running Claude's session view rather than a session of their own.

    Claude lists each session's process, interactive ones included, so a pane whose foreground
    process is one of them is that session. A pane without a readable pid cannot be told apart and
    is left to Rozi's own detection.
    """
    session_pids = {session.pid for session in sessions if session.pid is not None}
    return [
        pane
        for pane in panes
        if pane.runs_claude()
        and pane.foreground_pid is not None
        and pane.foreground_pid not in session_pids
    ]


def is_under(path: str | None, root: str | None) -> bool:
    if not path or not root:
        return False
    path = os.path.normpath(path)
    root = os.path.normpath(root)
    return path == root or path.startswith(root.rstrip(os.sep) + os.sep)


def pane_rows(pane: Pane, sessions: list[Session], scope: str) -> list[dict[str, object]]:
    """The rows one host pane publishes, oldest session first so rows keep their order.

    Interactive sessions are left out: each runs in a terminal of its own, which already shows
    up wherever it is.
    """
    chosen = [
        session
        for session in sessions
        if session.kind == "background"
        and (scope == "all" or is_under(session.cwd, pane.cwd))
    ]
    chosen.sort(key=lambda session: (session.started_at, session.session_id))
    return [session.row() for session in chosen]


def run_json(args: list[str]) -> str:
    try:
        result = subprocess.run(
            args,
            text=True,
            capture_output=True,
            stdin=subprocess.DEVNULL,
            timeout=COMMAND_TIMEOUT,
            check=False,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise SessionsError(f"{args[0]}: {error}") from error
    if result.returncode != 0:
        detail = text(result.stderr) or f"exit {result.returncode}"
        raise SessionsError(f"{' '.join(args[:3])} failed: {detail}")
    return result.stdout


def write_line(stream: Any, value: object) -> None:
    stream.write(json.dumps(value, separators=(",", ":")) + "\n")
    stream.flush()


@dataclass
class Publisher:
    token: int
    process: subprocess.Popen[str]
    last: list[dict[str, object]] | None = None


class SessionsService:
    def __init__(self, settings: Settings) -> None:
        self.settings = settings
        self.messages: queue.Queue[tuple[object, ...]] = queue.Queue()
        self.publishers: dict[int, Publisher] = {}
        self.next_token = 1
        self.reported_error: str | None = None

    def start_publisher(self, pane_id: int) -> Publisher:
        token = self.next_token
        self.next_token += 1
        environment = os.environ.copy()
        # `ROZI_PANE` names the pane a publish stream belongs to.
        environment["ROZI_PANE"] = str(pane_id)
        process = subprocess.Popen(
            [ROZI, "publish"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
            env=environment,
        )
        publisher = Publisher(token=token, process=process)
        self.publishers[pane_id] = publisher

        def drain() -> None:
            # Activations need no answer: Rozi focuses the pane itself, and Claude offers no way
            # to choose which session its client shows.
            assert process.stdout is not None
            for _line in process.stdout:
                pass
            process.wait()
            self.messages.put(("closed", pane_id, token))

        threading.Thread(target=drain, name=f"publish-{pane_id}", daemon=True).start()
        return publisher

    def stop_publisher(self, pane_id: int) -> None:
        publisher = self.publishers.pop(pane_id, None)
        if publisher is None:
            return
        try:
            if publisher.process.stdin is not None:
                publisher.process.stdin.close()
        except OSError:
            pass
        try:
            publisher.process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            publisher.process.kill()

    def publish(self, pane_id: int, rows: list[dict[str, object]]) -> None:
        publisher = self.publishers.get(pane_id) or self.start_publisher(pane_id)
        if publisher.last == rows:
            return
        try:
            assert publisher.process.stdin is not None
            write_line(publisher.process.stdin, {"rows": rows})
            publisher.last = rows
        except (BrokenPipeError, OSError):
            # The pane went away or the UI moved to another session. Try again next poll.
            self.stop_publisher(pane_id)

    def poll(self) -> None:
        sessions = parse_sessions(run_json([self.settings.claude, "agents", "--json"]))
        panes = parse_panes(run_json([ROZI, "list-panes", "--format", "json"]))
        wanted = {pane.pane_id: pane for pane in host_panes(panes, sessions)}
        for pane_id in set(self.publishers) - set(wanted):
            self.stop_publisher(pane_id)
        for pane in wanted.values():
            self.publish(pane.pane_id, pane_rows(pane, sessions, self.settings.scope))

    def report(self, error: str | None) -> None:
        # One line per distinct failure: a missing `claude` would otherwise log every poll.
        if error is not None and error != self.reported_error:
            print(error, file=sys.stderr, flush=True)
        self.reported_error = error

    def run(self) -> int:
        while True:
            try:
                self.poll()
                self.report(None)
            except SessionsError as error:
                self.report(str(error))
            try:
                message = self.messages.get(timeout=self.settings.poll_seconds)
            except queue.Empty:
                continue
            if message[0] == "stop":
                return 0
            if message[0] == "closed":
                pane_id, token = message[1], message[2]
                current = self.publishers.get(pane_id)  # type: ignore[arg-type]
                if current is not None and current.token == token:
                    self.publishers.pop(pane_id)  # type: ignore[arg-type]

    def close(self) -> None:
        for pane_id in list(self.publishers):
            self.stop_publisher(pane_id)


def main() -> int:
    if os.environ.get("ROZI_EXTENSION") != EXTENSION_ID:
        print(f"{EXTENSION_ID} must be launched by Rozi", file=sys.stderr)
        return 2
    service = SessionsService(Settings.from_environment(dict(os.environ)))

    def stop(_signum: int, _frame: object) -> None:
        service.messages.put(("stop",))

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        return service.run()
    finally:
        service.close()


if __name__ == "__main__":
    raise SystemExit(main())
