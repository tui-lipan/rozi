#!/usr/bin/env python3
"""Canonical long-running extension using only Rozi's public CLI streams."""

from __future__ import annotations

import argparse
import collections
import json
import os
import subprocess
import threading
import time
from pathlib import Path


ROZI = os.environ.get("ROZI_BIN", "rozi")
MAX_EVENTS = 40
events: collections.deque[dict[str, object]] = collections.deque(maxlen=MAX_EVENTS)
lock = threading.Lock()
state_path = Path(os.environ.get("ROZI_EXTENSION_DIR", ".")) / ".activity.json"


def load_events() -> None:
    try:
        stored = json.loads(state_path.read_text())
    except (OSError, json.JSONDecodeError):
        return
    if isinstance(stored, list):
        with lock:
            events.extend(event for event in stored if isinstance(event, dict))


def save_events() -> None:
    temporary = state_path.with_suffix(".tmp")
    try:
        with lock:
            payload = list(events)
        temporary.write_text(json.dumps(payload))
        temporary.replace(state_path)
    except OSError:
        pass


def event_label(event: dict[str, object]) -> str:
    kind = str(event.get("event", "event"))
    pane = event.get("pane")
    return f"{kind} · pane {pane}" if pane is not None else kind


def snapshot() -> list[str]:
    with lock:
        return [event_label(event) for event in reversed(events)]


def open_picker() -> None:
    lines = snapshot() or ["No activity reported yet"]
    subprocess.run(
        [ROZI, "pick", "--title", "Recent activity", "--placeholder", "Filter events…"],
        input="\n".join(lines) + "\n",
        text=True,
        check=False,
    )


def subscribe() -> None:
    process = subprocess.Popen(
        [ROZI, "subscribe"],
        stdout=subprocess.PIPE,
        text=True,
    )
    assert process.stdout is not None
    for line in process.stdout:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        with lock:
            events.append(event)
        save_events()
        if event.get("event") == "pane-status-changed" and event.get("status") == "blocked":
            subprocess.run(
                [
                    ROZI,
                    "notify",
                    event_label(event),
                    "--title",
                    "Blocked activity",
                    "--level",
                    "error",
                ],
                check=False,
            )


def rows() -> dict[str, object]:
    with lock:
        count = len(events)
        latest = events[-1] if events else None
    status = "blocked" if latest and latest.get("status") == "blocked" else "idle"
    return {
        "rows": [
            {
                "id": "recent",
                "status": status,
                "reason": event_label(latest) if latest else "waiting for events",
                "title": f"Recent activity ({count})",
            }
        ]
    }


def activation_reader(process: subprocess.Popen[str]) -> None:
    assert process.stdout is not None
    for line in process.stdout:
        try:
            activation = json.loads(line)
        except json.JSONDecodeError:
            continue
        if activation.get("activate") == "recent":
            open_picker()


def run_service() -> int:
    if os.environ.get("ROZI_EXTENSION") != "activity-dashboard":
        print("activity-dashboard must be launched by Rozi", file=os.sys.stderr)
        return 2
    threading.Thread(target=subscribe, daemon=True).start()
    publisher = subprocess.Popen(
        [ROZI, "publish"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert publisher.stdin is not None
    threading.Thread(target=activation_reader, args=(publisher,), daemon=True).start()
    while publisher.poll() is None:
        publisher.stdin.write(json.dumps(rows()) + "\n")
        publisher.stdin.flush()
        time.sleep(2)
    return publisher.returncode or 0


def main() -> int:
    load_events()
    parser = argparse.ArgumentParser()
    parser.add_argument("--service", action="store_true")
    parser.add_argument("--pick", action="store_true")
    args = parser.parse_args()
    if args.pick:
        open_picker()
        return 0
    if args.service:
        return run_service()
    parser.error("choose --service or --pick")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
