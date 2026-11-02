#!/usr/bin/env python3
"""Run a project's tests when an agent finishes a turn, with or without a UI attached.

`run-tests` is the action: one call runs the configured command once and answers with the result.
`watch` is the session service: it calls that action for each finished turn, asks the agent to fix
a failure, and tells you how it ended.

The service works from each agent's current state, not from the events it receives. An event only
wakes it. It then reads `agents list` and compares each agent's `completions` with the count it last
handled, so a turn that finished while the service was restarting is still noticed.
"""

from __future__ import annotations

import hashlib
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any

# A retry for a call that found the action busy waits for this long at most.
RECONCILE_SECONDS = 30
TAIL_CHARS = 2000
# State files outlive the session server they describe, so old ones are removed on start.
STATE_MAX_AGE_SECONDS = 30 * 24 * 3600
NOTIFY_TITLE = "Test loop"


@dataclass
class Settings:
    roots: list[str]
    command: list[str]
    max_attempts: int


def load_settings(raw: str | None) -> Settings:
    config = json.loads(raw or "{}")
    return Settings(
        roots=list(config.get("roots", [])),
        command=list(config.get("command", [])),
        max_attempts=int(config.get("max_attempts", 3)),
    )


def within_roots(path: str, roots: list[str]) -> bool:
    resolved = os.path.realpath(path)
    for root in roots:
        base = os.path.realpath(os.path.expanduser(root))
        try:
            if os.path.commonpath([resolved, base]) == base:
                return True
        except ValueError:  # Different drives on Windows.
            continue
    return False


def log_tail(path: Path) -> str:
    with path.open("rb") as log:
        log.seek(0, os.SEEK_END)
        log.seek(max(0, log.tell() - TAIL_CHARS * 4))
        return log.read().decode("utf-8", "replace")[-TAIL_CHARS:]


def run_tests(cwd: str, settings: Settings, state_dir: Path) -> dict[str, Any]:
    """Run the command once. A project it may not run in is an answer, not a failure of the call."""
    if not settings.command:
        return {"ran": False, "passed": False, "reason": "no `command` is configured"}
    if not within_roots(cwd, settings.roots):
        return {"ran": False, "passed": False, "reason": f"{cwd} is outside `roots`"}
    logs = state_dir / "logs"
    logs.mkdir(parents=True, exist_ok=True)
    log = logs / f"{hashlib.sha256(cwd.encode()).hexdigest()[:16]}.log"
    started = time.monotonic()
    try:
        with log.open("wb") as output:
            # stdout carries this action's one JSON result, so the command must not inherit it.
            exit_code = subprocess.run(
                settings.command,
                cwd=cwd,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.STDOUT,
                check=False,
            ).returncode
    except OSError as error:
        return {"ran": False, "passed": False, "reason": f"could not run the command: {error}"}
    return {
        "ran": True,
        "passed": exit_code == 0,
        "exit_code": exit_code,
        "duration_ms": int((time.monotonic() - started) * 1000),
        "log": str(log),
        "tail": log_tail(log),
    }


@dataclass
class Reply:
    ok: bool
    data: Any = None
    code: str | None = None
    error: str = ""


class Rozi:
    """The `rozi` commands the service uses. Tests replace this."""

    def __init__(self, program: str) -> None:
        self.program = program

    def call(self, *argv: str) -> Reply:
        done = subprocess.run(
            [self.program, *argv],
            stdin=subprocess.DEVNULL,
            capture_output=True,
            text=True,
            check=False,
        )
        try:
            reply = json.loads(done.stdout)
        except ValueError:
            return Reply(False, error=done.stderr.strip() or "no answer from rozi")
        return Reply(
            bool(reply.get("ok")),
            reply.get("data"),
            reply.get("code"),
            reply.get("error") or "",
        )

    def agents(self) -> list[dict[str, Any]] | None:
        reply = self.call("agents", "list", "--format", "json")
        return reply.data if reply.ok else None

    def run_tests(self, cwd: str) -> Reply:
        return self.call(
            "invoke", "test-loop.run-tests", "--args-json", json.dumps({"cwd": cwd}), "--format", "json"
        )

    def prompt(self, reference: dict[str, Any], text: str) -> Reply:
        return self.call("agents", "prompt", "--ref", json.dumps(reference), "--format", "json", text)

    def notify(self, message: str, level: str = "info") -> None:
        self.call("notify", message, "--title", NOTIFY_TITLE, "--level", level)


class AgentLedger:
    """What has been handled for each agent, kept per session server.

    An agent's reference and its count mean nothing to another server instance, so a restarted
    server starts a new file. A service restarted under the same server picks up where it left off.
    """

    def __init__(self, path: Path) -> None:
        self.path = path
        try:
            self.agents: dict[str, dict[str, Any]] = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            self.agents = {}

    def save(self) -> None:
        self.path.parent.mkdir(parents=True, exist_ok=True)
        handle, temporary = tempfile.mkstemp(dir=self.path.parent, prefix=".ledger-")
        with os.fdopen(handle, "w", encoding="utf-8") as output:
            json.dump(self.agents, output)
        os.replace(temporary, self.path)


def fix_prompt(result: dict[str, Any], command: list[str], attempt: int, limit: int) -> str:
    """One line that points at the output, so a test's output never gets typed into the agent."""
    return (
        f"The test command `{' '.join(command)}` failed with exit code {result.get('exit_code')} "
        f"(fix attempt {attempt} of {limit}). Its output is in {result.get('log')}. "
        "Find the cause and fix it."
    )


class Loop:
    def __init__(self, rozi: Rozi, settings: Settings, ledger: AgentLedger) -> None:
        self.rozi = rozi
        self.settings = settings
        self.ledger = ledger

    def reconcile(self) -> None:
        """Handle every agent with a finished turn it has not handled yet."""
        agents = self.rozi.agents()
        if agents is None:
            return  # Asked again on the next wake.
        current = {json.dumps(agent["ref"], sort_keys=True): agent for agent in agents}
        stale = self.ledger.agents.keys() - current.keys()
        for key in stale:
            del self.ledger.agents[key]
        if stale:
            self.ledger.save()
        for key, agent in current.items():
            record = self.ledger.agents.setdefault(key, {"seen": 0, "attempts": 0, "gave_up": False})
            # An agent that is working again, or waiting on someone, comes back here when it stops.
            if agent["completions"] > record["seen"] and agent["state"] in ("done", "idle"):
                self.handle_turn(agent, record)

    def handle_turn(self, agent: dict[str, Any], record: dict[str, Any]) -> None:
        cwd = agent.get("cwd")
        if not cwd or not within_roots(cwd, self.settings.roots):
            self.finish(agent, record)
            return
        name = Path(cwd).name
        reply = self.rozi.run_tests(cwd)
        if reply.code == "action-busy" or reply.code == "invocation-limit":
            return  # Not handled yet: the next wake tries again.
        self.finish(agent, record)
        if not reply.ok:
            self.rozi.notify(f"{name}: could not run the tests: {reply.error}", "error")
            return
        result = reply.data["result"]
        if not result["ran"]:
            self.rozi.notify(f"{name}: {result.get('reason')}", "error")
        elif result["passed"]:
            self.on_pass(name, record)
        else:
            self.on_failure(agent, record, name, result)

    def finish(self, agent: dict[str, Any], record: dict[str, Any]) -> None:
        record["seen"] = agent["completions"]
        self.ledger.save()

    def on_pass(self, name: str, record: dict[str, Any]) -> None:
        attempts = record["attempts"]
        record.update(attempts=0, gave_up=False)
        self.ledger.save()
        after = f" after {attempts} fix attempts" if attempts else ""
        self.rozi.notify(f"{name}: tests pass{after}")

    def on_failure(
        self, agent: dict[str, Any], record: dict[str, Any], name: str, result: dict[str, Any]
    ) -> None:
        limit = self.settings.max_attempts
        if record["attempts"] >= limit:
            if not record["gave_up"]:
                record["gave_up"] = True
                self.ledger.save()
                self.rozi.notify(
                    f"{name}: tests still fail after {record['attempts']} fix attempts. "
                    f"Output: {result.get('log')}",
                    "error",
                )
            return
        # Count the attempt first: a crash here must not type the same request into an agent twice.
        record["attempts"] += 1
        self.ledger.save()
        prompt = fix_prompt(result, self.settings.command, record["attempts"], limit)
        reply = self.rozi.prompt(agent["ref"], prompt)
        if reply.ok:
            return
        # A code is rozi refusing, so nothing was typed and the attempt is unspent. Without one, the
        # request may have got through, so it stays counted. A person may have started another
        # turn either way, and the tests run again when it ends.
        if reply.code is None:
            self.rozi.notify(f"{name}: tests fail, and asking the agent got no answer: {reply.error}", "error")
            return
        record["attempts"] -= 1
        self.ledger.save()
        self.rozi.notify(f"{name}: tests fail, and the agent was not asked: {reply.error}", "error")


class Stream:
    """`rozi subscribe`, drained on its own thread so a long test run never makes it fall behind."""

    def __init__(self, program: str, wake: threading.Event) -> None:
        self.wake = wake
        self.process = subprocess.Popen(
            [program, "subscribe", "agent-state-changed", "--snapshot"],
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            text=True,
        )
        threading.Thread(target=self.drain, daemon=True).start()

    def drain(self) -> None:
        assert self.process.stdout is not None
        for line in self.process.stdout:
            if should_wake(line):
                self.wake.set()
        self.process.wait()
        self.wake.set()

    def exit_status(self) -> int | None:
        return self.process.poll()


def should_wake(line: str) -> bool:
    """The first record, a snapshot, proves the stream is live. After it, only a turn's end counts."""
    try:
        record = json.loads(line)
    except ValueError:
        return False
    if record.get("type") == "snapshot":
        return True
    return record.get("type") == "event" and record.get("data", {}).get("state") in ("done", "idle")


def prune_ledgers(state_dir: Path, now: float) -> None:
    for path in state_dir.glob("agents-*.json"):
        try:
            if now - path.stat().st_mtime > STATE_MAX_AGE_SECONDS:
                path.unlink()
        except OSError:
            continue


def watch(environ: dict[str, str]) -> int:
    """Run until the stream stops, and exit with its status so the service restarts when it should."""
    program = environ["ROZI_BIN"]
    state_dir = Path(environ["ROZI_EXTENSION_STATE_DIR"])
    prune_ledgers(state_dir, time.time())
    ledger = AgentLedger(state_dir / f"agents-{environ['ROZI_SESSION_INSTANCE']}.json")
    loop = Loop(Rozi(program), load_settings(environ.get("ROZI_EXTENSION_CONFIG")), ledger)
    wake = threading.Event()
    stream = Stream(program, wake)
    # Wait for the snapshot, so a turn ending from here on also raises an event this will read.
    wake.wait()
    while True:
        wake.clear()
        status = stream.exit_status()
        if status is not None:
            return status
        loop.reconcile()
        wake.wait(RECONCILE_SECONDS)


def action(environ: dict[str, str]) -> int:
    args = json.load(sys.stdin)
    state_dir = Path(environ["ROZI_EXTENSION_STATE_DIR"])
    result = run_tests(args["cwd"], load_settings(environ.get("ROZI_EXTENSION_CONFIG")), state_dir)
    print(json.dumps(result))
    return 0


def main(argv: list[str]) -> int:
    commands = {"run-tests": action, "watch": watch}
    if len(argv) != 2 or argv[1] not in commands:
        print("usage: test_loop.py run-tests|watch", file=sys.stderr)
        return 2
    return commands[argv[1]](dict(os.environ))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
