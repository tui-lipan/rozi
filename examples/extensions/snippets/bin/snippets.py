#!/usr/bin/env python3
"""Saved-command picker. Paste a row into the focused pane, or add one from a nested prompt."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import uuid
from dataclasses import dataclass
from pathlib import Path


ROZI = os.environ.get("ROZI_BIN") or "rozi"
EXTENSION_ID = "snippets"
STATE_FILE = "commands.json"


class ToolError(RuntimeError):
    """A concise user-facing command failure."""


@dataclass(frozen=True)
class Snippet:
    id: str
    text: str
    saved: bool


def settings() -> dict:
    try:
        raw = json.loads(os.environ.get("ROZI_EXTENSION_CONFIG") or "{}")
    except json.JSONDecodeError:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    commands = raw.get("commands") or []
    if not isinstance(commands, list):
        commands = []
    return {
        "submit": bool(raw.get("submit", False)),
        "commands": [item for item in commands if isinstance(item, str)],
    }


def state_path() -> Path | None:
    base = os.environ.get("XDG_STATE_HOME")
    if not base:
        if os.name == "nt":
            base = os.environ.get("LOCALAPPDATA") or str(Path.home() / "AppData" / "Local")
        else:
            base = str(Path.home() / ".local" / "state")
    directory = Path(base) / "rozi-snippets"
    try:
        directory.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None
    return directory / STATE_FILE


def load_saved() -> list[Snippet]:
    path = state_path()
    if path is None or not path.is_file():
        return []
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return []
    items = raw.get("commands") if isinstance(raw, dict) else None
    if not isinstance(items, list):
        return []
    snippets = []
    for item in items:
        if not isinstance(item, dict):
            continue
        snippet_id = item.get("id")
        text = item.get("text")
        if isinstance(snippet_id, str) and snippet_id and isinstance(text, str) and text.strip():
            snippets.append(Snippet(snippet_id, text, True))
    return snippets


def save_saved(snippets: list[Snippet]) -> None:
    path = state_path()
    if path is None:
        raise ToolError("Could not write snippet state")
    payload = {
        "commands": [{"id": item.id, "text": item.text} for item in snippets if item.saved]
    }
    encoded = json.dumps(payload, ensure_ascii=False, indent=2) + "\n"
    temporary = path.with_name(path.name + ".tmp")
    try:
        temporary.write_text(encoded, encoding="utf-8")
        temporary.replace(path)
    except OSError as error:
        try:
            temporary.unlink(missing_ok=True)
        except OSError:
            pass
        raise ToolError("Could not write snippet state") from error


def all_snippets(config: dict) -> list[Snippet]:
    snippets = []
    for index, text in enumerate(config["commands"]):
        stripped = text.strip()
        if stripped:
            snippets.append(Snippet(f"config:{index}", stripped, False))
    snippets.extend(load_saved())
    return snippets


def new_id() -> str:
    return uuid.uuid4().hex[:12]


def valid_text(value: object) -> str:
    text = str(value).strip()
    if not text:
        raise ToolError("Command is empty")
    if "\n" in text or "\r" in text:
        raise ToolError("Command cannot contain a newline")
    return text


def add_snippet(value: object) -> Snippet:
    text = valid_text(value)
    snippets = load_saved()
    if any(item.text == text for item in snippets):
        raise ToolError("Command already saved")
    snippet = Snippet(new_id(), text, True)
    snippets.append(snippet)
    save_saved(snippets)
    return snippet


def delete_snippet(selected: object, items: dict[str, Snippet]) -> None:
    snippet = items.get(str(selected))
    if snippet is None:
        raise ToolError("Nothing to delete")
    if not snippet.saved:
        raise ToolError("Config snippets cannot be deleted here")
    save_saved([item for item in load_saved() if item.id != snippet.id])


def rows(snippets: list[Snippet]) -> list[dict[str, object]]:
    return [
        {
            "id": item.id,
            "label": item.text,
            "group": "From config" if not item.saved else "Saved",
        }
        for item in snippets
    ]


def picker_request(snippets: list[Snippet]) -> dict[str, object]:
    return {
        "title": "Snippets",
        "placeholder": "Filter…",
        "empty": "No snippets yet",
        "actions": [
            {
                "id": "create",
                "key": "ctrl-n",
                "label": "new",
                "prompt": {
                    "title": "Command",
                    "placeholder": "git status",
                },
            },
            {
                "id": "delete",
                "key": "ctrl-d",
                "label": "delete",
                "confirm": True,
            },
        ],
        "rows": rows(snippets),
    }


def notify_error(message: str) -> None:
    try:
        subprocess.run(
            [ROZI, "notify", message, "--title", "Snippets", "--level", "error"],
            check=False,
        )
    except OSError:
        print(message, file=sys.stderr)


def paste(text: str, submit: bool) -> None:
    try:
        send = subprocess.run([ROZI, "send-text", text], capture_output=True, text=True)
    except OSError as error:
        raise ToolError("Rozi command unavailable") from error
    if send.returncode != 0:
        detail = (send.stderr or send.stdout).strip()
        raise ToolError(detail.splitlines()[-1] if detail else "Could not paste")
    if not submit:
        return
    try:
        keys = subprocess.run([ROZI, "send-keys", "Enter"], capture_output=True, text=True)
    except OSError as error:
        raise ToolError("Rozi command unavailable") from error
    if keys.returncode != 0:
        detail = (keys.stderr or keys.stdout).strip()
        raise ToolError(detail.splitlines()[-1] if detail else "Could not submit")


def apply_picker_event(
    event: dict[str, object],
    items: dict[str, Snippet],
) -> tuple[str, str | None]:
    if event.get("cancelled"):
        return "done", None
    selected = event.get("selected")
    action = event.get("action")
    if action == "create" and event.get("input") is not None:
        add_snippet(event["input"])
        return "refresh", None
    if action == "delete" and selected is not None:
        delete_snippet(selected, items)
        return "refresh", None
    if action is None and selected is not None:
        snippet = items.get(str(selected))
        if snippet is None:
            raise ToolError("Snippet changed; refresh")
        return "paste", snippet.text
    return "ignore", None


def run_picker(config: dict) -> int:
    snippets = all_snippets(config)
    items = {item.id: item for item in snippets}
    try:
        process = subprocess.Popen(
            [ROZI, "pick", "--json"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            encoding="utf-8",
            errors="replace",
        )
    except OSError:
        raise ToolError("Rozi picker unavailable")
    assert process.stdin is not None
    assert process.stdout is not None
    assert process.stderr is not None

    stream_open = True
    terminal_event = False

    def send(payload: dict[str, object]) -> bool:
        nonlocal stream_open
        if not stream_open:
            return False
        try:
            process.stdin.write(json.dumps(payload, ensure_ascii=False) + "\n")
            process.stdin.flush()
            return True
        except (BrokenPipeError, OSError, ValueError):
            stream_open = False
            return False

    def refresh() -> bool:
        nonlocal snippets, items
        snippets = all_snippets(config)
        items = {item.id: item for item in snippets}
        return send({"rows": rows(snippets)})

    if not send(picker_request(snippets)):
        stream_open = False
    try:
        while stream_open:
            line = process.stdout.readline()
            if not line:
                break
            try:
                event = json.loads(line)
            except json.JSONDecodeError:
                continue
            if not isinstance(event, dict):
                continue
            try:
                outcome, text = apply_picker_event(event, items)
            except ToolError as error:
                notify_error(str(error))
                if not refresh():
                    break
                continue
            if outcome == "done":
                terminal_event = True
                break
            if outcome == "refresh":
                if not refresh():
                    break
                continue
            if outcome == "paste" and text is not None:
                terminal_event = True
                try:
                    paste(text, config["submit"])
                except ToolError as error:
                    notify_error(str(error))
                break
    finally:
        if stream_open:
            try:
                process.stdin.close()
            except (BrokenPipeError, OSError, ValueError):
                pass
        return_code = process.wait()

    if return_code not in {0, 1} or (return_code == 1 and not terminal_event):
        detail = process.stderr.read().strip()
        notify_error(detail.splitlines()[-1] if detail else "Picker closed")
    return 0


def main() -> int:
    if os.environ.get("ROZI_EXTENSION") != EXTENSION_ID:
        print("snippets must be launched by Rozi", file=sys.stderr)
        return 2
    mode = sys.argv[1] if len(sys.argv) > 1 else "pick"
    if mode != "pick":
        print(f"unknown mode: {mode}", file=sys.stderr)
        return 2
    try:
        return run_picker(settings())
    except ToolError as error:
        notify_error(str(error))
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
