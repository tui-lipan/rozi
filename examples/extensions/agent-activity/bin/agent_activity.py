#!/usr/bin/env python3
"""Notify on public pane statuses and list them in a picker.

rozi already lists every pane's `rozi status` in its Activity sidebar, so this example adds what the
sidebar does not: a notification on each new blocked, error, or finished state, and a searchable
picker of every reporting pane.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import signal
import subprocess
import sys
import threading
import time
from dataclasses import dataclass
from typing import Any


ROZI = os.environ.get("ROZI_BIN", "rozi")
EXTENSION_ID = "agent-activity"
EVENTS = (
    "pane-status-changed",
    "pane-exited",
    "config-reloaded",
)
REFRESH_SECONDS = 30.0


class ActivityError(RuntimeError):
    """A public CLI operation or stream failed."""


@dataclass
class Pane:
    pane_id: int
    title: str
    workspace: int
    terminal_status: str
    reported_status: str | None
    reason: str | None

    @classmethod
    def from_wire(cls, value: dict[str, Any]) -> Pane | None:
        try:
            pane_id = int(value["id"])
        except (KeyError, TypeError, ValueError):
            return None
        reported_status = clean(value.get("reported_status"))
        reason = clean(value.get("status_reason"))
        return cls(
            pane_id=pane_id,
            title=clean(value.get("title")) or f"Pane {pane_id}",
            workspace=integer(value.get("workspace"), 0),
            terminal_status=clean(value.get("status")) or "",
            reported_status=reported_status,
            reason=reason,
        )

    def is_live_activity(self) -> bool:
        terminal = self.terminal_status.casefold()
        return bool(self.reported_status) and not (
            terminal.startswith("exited ") or terminal.startswith("error:")
        )




def clean(value: object) -> str | None:
    if value is None:
        return None
    text = str(value).strip()
    return text or None


def integer(value: object, default: int) -> int:
    try:
        return int(value)  # type: ignore[arg-type]
    except (TypeError, ValueError):
        return default


def row_id(pane_id: int) -> str:
    return f"pane:{pane_id}"


def pane_id_from_row(value: object) -> int | None:
    text = clean(value)
    if not text or not text.startswith("pane:"):
        return None
    try:
        return int(text.removeprefix("pane:"))
    except ValueError:
        return None


def rozi_json(*args: str) -> Any:
    result = subprocess.run(
        [ROZI, *args],
        text=True,
        capture_output=True,
        check=False,
    )
    if result.returncode != 0:
        detail = clean(result.stderr) or clean(result.stdout) or f"exit {result.returncode}"
        raise ActivityError(f"rozi {' '.join(args)} failed: {detail}")
    try:
        response = json.loads(result.stdout)
    except json.JSONDecodeError as error:
        raise ActivityError(f"rozi {' '.join(args)} returned invalid JSON") from error
    if not isinstance(response, dict) or response.get("ok") is not True:
        detail = response.get("error") if isinstance(response, dict) else None
        raise ActivityError(str(detail or f"rozi {' '.join(args)} failed"))
    return response.get("data")


def list_panes() -> dict[int, Pane]:
    data = rozi_json("list-panes")
    if not isinstance(data, list):
        raise ActivityError("rozi list-panes returned a non-list payload")
    panes: dict[int, Pane] = {}
    for item in data:
        if isinstance(item, dict) and (pane := Pane.from_wire(item)) is not None:
            panes[pane.pane_id] = pane
    return panes


def notify(message: str, *, error: bool = False) -> None:
    args = [ROZI, "notify"]
    if error:
        args.extend(["--title", "Agent activity", "--level", "error"])
    args.extend(["--", message[:240]])
    subprocess.run(
        args,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.DEVNULL,
        check=False,
    )


def focus_pane(pane_id: int) -> bool:
    result = subprocess.run(
        [ROZI, "focus", str(pane_id)],
        text=True,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.PIPE,
        check=False,
    )
    if result.returncode != 0:
        notify(clean(result.stderr) or f"Pane {pane_id} is no longer available", error=True)
        return False
    return True


def status_group(status: str | None) -> tuple[int, str]:
    normalized = (status or "").strip().casefold()
    if normalized == "blocked":
        return (0, "Blocked")
    if is_error_status(normalized):
        return (1, "Errors")
    if normalized == "working":
        return (2, "Working")
    if normalized in {"done", "finished", "complete", "completed"}:
        return (3, "Finished")
    if normalized == "idle":
        return (4, "Idle")
    return (5, "Other")


def picker_rows(panes: dict[int, Pane]) -> list[dict[str, object]]:
    active = [pane for pane in panes.values() if pane.is_live_activity()]
    active.sort(
        key=lambda pane: (
            status_group(pane.reported_status)[0],
            pane.workspace,
            pane.pane_id,
        )
    )
    if not active:
        return [
            {
                "id": "empty",
                "label": "No reported activity",
                "description": "rozi status",
                "disabled": "No status rows",
            }
        ]
    rows = []
    for pane in active:
        _, group = status_group(pane.reported_status)
        label = pane.reason or pane.title
        location = f"pane {pane.pane_id}"
        if pane.workspace:
            location += f" · ws {pane.workspace}"
        rows.append(
            {
                "id": row_id(pane.pane_id),
                "label": label,
                "description": f"{pane.reported_status} · {location}",
                "group": group,
            }
        )
    return rows


def picker_request(panes: dict[int, Pane]) -> dict[str, object]:
    return {
        "title": "Agent activity",
        "placeholder": "Filter activity…",
        "width": 76,
        "actions": [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {"id": "focus", "key": "f", "label": "focus", "close": True},
        ],
        "rows": picker_rows(panes),
    }


def write_line(stream: Any, payload: dict[str, object]) -> None:
    stream.write(json.dumps(payload, separators=(",", ":")) + "\n")
    stream.flush()


def wait_process(process: subprocess.Popen[str], *, graceful: bool) -> None:
    if process.poll() is not None:
        return
    if graceful and process.stdin is not None:
        try:
            process.stdin.close()
        except OSError:
            pass
        try:
            process.wait(timeout=1.0)
            return
        except subprocess.TimeoutExpired:
            pass
    process.terminate()
    try:
        process.wait(timeout=1.0)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait()


def run_picker() -> int:
    try:
        panes = list_panes()
    except ActivityError as error:
        notify(str(error), error=True)
        return 1
    process = subprocess.Popen(
        [ROZI, "pick", "--json"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        bufsize=1,
    )
    if process.stdin is None or process.stdout is None:
        wait_process(process, graceful=False)
        return 1
    cancelled = False
    try:
        write_line(process.stdin, picker_request(panes))
        for line in process.stdout:
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict):
                break
            if event.get("cancelled"):
                cancelled = True
                break
            action = event.get("action")
            if action == "refresh":
                try:
                    panes = list_panes()
                    write_line(process.stdin, {"rows": picker_rows(panes)})
                except ActivityError as error:
                    notify(str(error), error=True)
                continue
            pane_id = pane_id_from_row(event.get("selected"))
            if pane_id is not None and (action == "focus" or "selected" in event):
                focus_pane(pane_id)
                break
    except (BrokenPipeError, OSError):
        pass
    finally:
        wait_process(process, graceful=True)
    return 0 if cancelled else process.returncode or 0


def is_error_status(status: str) -> bool:
    normalized = status.strip().casefold()
    return normalized in {"error", "errored", "failed", "failure"} or normalized.startswith(
        ("error:", "failed:")
    )


def transition_kind(previous: str | None, current: str | None) -> str | None:
    before = (previous or "").strip().casefold()
    after = (current or "").strip().casefold()
    if after == "blocked" and before != "blocked":
        return "blocked"
    if is_error_status(after) and not is_error_status(before):
        return "error"
    finished = {"done", "finished", "complete", "completed"}
    if after in finished and before not in finished:
        return "finished"
    if after == "idle" and before in {"working", "blocked"}:
        return "finished"
    return None


class ActivityService:
    def __init__(self) -> None:
        self.messages: queue.Queue[tuple[object, ...]] = queue.Queue()
        self.panes: dict[int, Pane] = {}
        self.subscription: subprocess.Popen[str] | None = None

    def start_subscription(self) -> None:
        process = subprocess.Popen(
            [ROZI, "subscribe", *EVENTS],
            stdout=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        if process.stdout is None:
            wait_process(process, graceful=False)
            raise ActivityError("could not read the rozi subscribe stream")
        self.subscription = process

        def read() -> None:
            assert process.stdout is not None
            try:
                for line in process.stdout:
                    try:
                        event = json.loads(line)
                    except json.JSONDecodeError:
                        continue
                    if isinstance(event, dict):
                        self.messages.put(("event", event))
            finally:
                returncode = process.wait()
                self.messages.put(("subscription-closed", returncode))

        threading.Thread(target=read, name="agent-activity-subscribe", daemon=True).start()

    def refresh(self) -> None:
        self.panes = list_panes()

    def status_changed(self, data: dict[str, Any]) -> None:
        pane_id = integer(data.get("pane"), -1)
        if pane_id < 0:
            return
        pane = self.panes.get(pane_id)
        if pane is None:
            self.refresh()
            pane = self.panes.get(pane_id)
            if pane is None:
                return
        previous = clean(data.get("previous_status"))
        current = clean(data.get("status"))
        pane.reported_status = current
        pane.reason = clean(data.get("reason"))

        kind = transition_kind(previous, current)
        if kind is None:
            return
        detail = f": {pane.reason}" if pane.reason else ""
        if kind == "blocked":
            notify(f"Pane {pane_id} blocked{detail}", error=True)
        elif kind == "error":
            notify(f"Pane {pane_id} error{detail}", error=True)
        else:
            notify(f"Pane {pane_id} finished{detail}")

    def pane_exited(self, data: dict[str, Any]) -> None:
        pane_id = integer(data.get("pane"), -1)
        if pane_id < 0:
            return
        pane = self.panes.pop(pane_id, None)
        code = integer(data.get("code"), 0)
        if (
            pane is not None
            and pane.reported_status
            and code != 0
            and not is_error_status(pane.reported_status)
        ):
            notify(f"Pane {pane_id} exited with code {code}", error=True)

    def handle_event(self, event: dict[str, Any]) -> None:
        kind = clean(event.get("event"))
        raw_data = event.get("data")
        data = raw_data if isinstance(raw_data, dict) else {}
        if kind == "pane-status-changed":
            self.status_changed(data)
        elif kind == "pane-exited":
            self.pane_exited(data)
        elif kind == "config-reloaded":
            self.refresh()

    def run(self) -> int:
        self.start_subscription()
        self.refresh()
        next_refresh = time.monotonic() + REFRESH_SECONDS
        while True:
            timeout = max(0.0, next_refresh - time.monotonic())
            try:
                message = self.messages.get(timeout=timeout)
            except queue.Empty:
                message = ("tick",)

            kind = message[0]
            if kind == "stop":
                return 0
            if kind == "subscription-closed":
                raise ActivityError(
                    f"rozi subscribe stream closed (exit {integer(message[1], 1)})"
                )
            if kind == "event" and isinstance(message[1], dict):
                self.handle_event(message[1])

            now = time.monotonic()
            if now >= next_refresh:
                self.refresh()
                next_refresh = now + REFRESH_SECONDS

    def close(self) -> None:
        if self.subscription is not None:
            wait_process(self.subscription, graceful=False)
            self.subscription = None


def run_service() -> int:
    if os.environ.get("ROZI_EXTENSION") != EXTENSION_ID:
        print(f"{EXTENSION_ID} must be launched by Rozi", file=sys.stderr)
        return 2
    service = ActivityService()

    def stop(_signum: int, _frame: object) -> None:
        service.messages.put(("stop",))

    signal.signal(signal.SIGTERM, stop)
    signal.signal(signal.SIGINT, stop)
    try:
        return service.run()
    except ActivityError as error:
        print(error, file=sys.stderr)
        return 1
    finally:
        service.close()


def main() -> int:
    parser = argparse.ArgumentParser()
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--service", action="store_true")
    mode.add_argument("--pick", action="store_true")
    args = parser.parse_args()
    return run_service() if args.service else run_picker()


if __name__ == "__main__":
    raise SystemExit(main())
