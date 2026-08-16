#!/usr/bin/env python3
"""Git pickers implemented with Python's stdlib, Git, and Rozi's public CLI."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable


ROZI = os.environ.get("ROZI_BIN", "rozi")
PROTECTED_BRANCHES = {"main", "master", "trunk"}


class ToolError(RuntimeError):
    """A concise user-facing command failure."""


def decode(value: bytes) -> str:
    return os.fsdecode(value)


def concise_error(error: subprocess.CalledProcessError) -> str:
    stderr = decode(error.stderr or b"")
    lines = [line.strip() for line in stderr.splitlines() if line.strip()]
    detail = lines[-1] if lines else f"Git exited {error.returncode}"
    for prefix in ("fatal: ", "error: "):
        if detail.lower().startswith(prefix):
            detail = detail[len(prefix) :]
            break
    return detail


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[bytes]:
    try:
        return subprocess.run(
            ["git", *args],
            capture_output=True,
            check=check,
        )
    except subprocess.CalledProcessError as error:
        raise ToolError(concise_error(error)) from error
    except OSError as error:
        raise ToolError("Git not found") from error


def notify_error(message: str) -> None:
    try:
        subprocess.run(
            [
                ROZI,
                "notify",
                message,
                "--title",
                "Git tools",
                "--level",
                "error",
            ],
            check=False,
        )
    except OSError:
        print(message, file=sys.stderr)


def normalized_path(path: str) -> str:
    return os.path.normcase(os.path.realpath(path))


def repository_root() -> str:
    result = git("rev-parse", "--show-toplevel", check=False)
    if result.returncode != 0:
        raise ToolError("Not a Git worktree")
    return decode(result.stdout).rstrip("\r\n")


def dirty_worktree(path: str | None = None) -> bool:
    prefix = ("-C", path) if path is not None else ()
    result = git(
        *prefix,
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=normal",
        check=False,
    )
    return result.returncode != 0 or bool(result.stdout)


def valid_new_branch(name: str) -> str:
    name = name.strip()
    if not name:
        raise ToolError("Branch name is empty")
    if git("check-ref-format", "--branch", name, check=False).returncode != 0:
        raise ToolError("Invalid branch name")
    ref = f"refs/heads/{name}"
    if git("show-ref", "--verify", "--quiet", ref, check=False).returncode == 0:
        raise ToolError("Branch already exists")
    return name


def protected_refs() -> set[str]:
    protected = {f"refs/heads/{name}" for name in PROTECTED_BRANCHES}
    output = git(
        "for-each-ref",
        "refs/remotes/",
        "--format=%(symref)%00",
    ).stdout
    for raw_target in output.split(b"\0"):
        target = decode(raw_target).strip()
        parts = target.split("/", 3)
        if len(parts) == 4 and parts[:2] == ["refs", "remotes"]:
            protected.add(f"refs/heads/{parts[3]}")
    return protected


@dataclass(frozen=True)
class Branch:
    ref: str
    name: str
    current: bool
    age: str
    worktree: str | None
    protected: bool


def branches() -> list[Branch]:
    output = git(
        "for-each-ref",
        "refs/heads/",
        "--sort=-committerdate",
        "--format=%(refname)%00%(refname:lstrip=2)%00%(HEAD)%00"
        "%(committerdate:relative)%00%(worktreepath)%00END%00",
    ).stdout
    protected = protected_refs()
    result = []
    fields = output.split(b"\0")
    for offset in range(0, len(fields) - 5, 6):
        record = fields[offset : offset + 6]
        record[0] = record[0].removeprefix(b"\n")
        if record[-1] != b"END":
            continue
        ref, name, marker, age, worktree = map(decode, record[:-1])
        result.append(
            Branch(
                ref=ref,
                name=name,
                current=marker == "*",
                age=age,
                worktree=worktree or None,
                protected=ref in protected,
            )
        )
    return result


def branch_rows() -> tuple[list[dict[str, object]], dict[str, Branch]]:
    dirty = dirty_worktree()
    items = branches()
    rows: list[dict[str, object]] = []
    by_ref = {branch.ref: branch for branch in items}
    for branch in items:
        if branch.current:
            group = "Current"
            disabled = "Current · dirty" if dirty else "Current"
        elif branch.worktree:
            group = "Other worktrees"
            disabled = f"Open in {Path(branch.worktree).name}"
        elif branch.protected:
            group = "Protected"
            disabled = "Dirty tree" if dirty else None
        else:
            group = "Recent"
            disabled = "Dirty tree" if dirty else None

        description = branch.age
        if branch.protected:
            description = f"protected · {description}"
        row: dict[str, object] = {
            "id": branch.ref,
            "label": branch.name,
            "group": group,
            "description": description,
            "active": branch.current,
        }
        if disabled:
            row["disabled"] = disabled
        rows.append(row)
    return rows, by_ref


@dataclass(frozen=True)
class Worktree:
    path: str
    branch: str
    current: bool
    primary: bool
    dirty: bool
    locked: bool
    prunable: bool


def worktrees() -> list[Worktree]:
    current_root = normalized_path(repository_root())
    records = git("worktree", "list", "--porcelain", "-z").stdout.split(b"\0\0")
    result = []
    for index, record in enumerate(records):
        if not record:
            continue
        fields: dict[str, str] = {}
        flags: set[str] = set()
        for raw_field in record.split(b"\0"):
            key, separator, value = raw_field.partition(b" ")
            name = decode(key)
            if separator:
                fields[name] = decode(value)
            else:
                flags.add(name)
        path = fields.get("worktree")
        if not path:
            continue
        ref = fields.get("branch")
        branch = (
            ref.removeprefix("refs/heads/")
            if ref
            else "bare"
            if "bare" in flags
            else "detached"
        )
        prunable = "prunable" in fields or "prunable" in flags
        result.append(
            Worktree(
                path=path,
                branch=branch,
                current=normalized_path(path) == current_root,
                primary=index == 0,
                dirty=False if prunable else dirty_worktree(path),
                locked="locked" in fields or "locked" in flags,
                prunable=prunable,
            )
        )
    return result


def worktree_rows() -> tuple[list[dict[str, object]], dict[str, Worktree]]:
    items = worktrees()
    rows: list[dict[str, object]] = []
    by_path = {item.path: item for item in items}
    for item in items:
        states = []
        if item.primary:
            states.append("primary")
        if item.dirty:
            states.append("dirty")
        if item.locked:
            states.append("locked")
        description = " · ".join([item.branch, *states])
        row: dict[str, object] = {
            "id": item.path,
            "label": item.path,
            "group": "Current" if item.current else "Other worktrees",
            "description": description,
            "active": item.current,
        }
        if item.current:
            row["disabled"] = "Current"
        elif item.prunable:
            row["disabled"] = "Missing"
        rows.append(row)
    return rows, by_path


def picker_request(mode: str, rows: list[dict[str, object]]) -> dict[str, object]:
    if mode == "branches":
        title = "Git branches"
        actions = [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {
                "id": "create",
                "key": "ctrl-n",
                "label": "new branch",
                "prompt": "New branch",
            },
            {
                "id": "delete",
                "key": "ctrl-d",
                "label": "delete",
                "confirm": True,
            },
        ]
    else:
        title = "Git worktrees"
        actions = [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {
                "id": "create",
                "key": "ctrl-n",
                "label": "new worktree",
                "prompt": "New worktree branch",
            },
            {
                "id": "remove",
                "key": "ctrl-d",
                "label": "remove",
                "confirm": True,
            },
        ]
    return {
        "title": title,
        "placeholder": "Filter…",
        "width": 84,
        "actions": actions,
        "rows": rows,
    }


def create_branch(value: object) -> None:
    name = valid_new_branch(str(value))
    git("branch", name, "HEAD")


def delete_branch(selected: object, items: dict[str, Branch]) -> None:
    ref = str(selected)
    branch = items.get(ref)
    if branch is None:
        raise ToolError("Branch changed; refresh")
    if branch.current:
        raise ToolError("Current branch")
    if branch.protected:
        raise ToolError("Protected branch")
    if branch.worktree:
        raise ToolError("Branch is in another worktree")
    git("branch", "-d", "--", branch.name)


def create_worktree(value: object, items: dict[str, Worktree]) -> None:
    name = valid_new_branch(str(value))
    primary = next((item for item in items.values() if item.primary), None)
    if primary is None:
        raise ToolError("Primary worktree not found")
    suffix = name.replace("/", "-").replace("\\", "-")
    path = str(Path(primary.path).parent / f"{Path(primary.path).name}-{suffix}")
    if os.path.lexists(path):
        raise ToolError("Worktree path already exists")
    git("worktree", "add", "-b", name, "--", path)


def remove_worktree(selected: object, items: dict[str, Worktree]) -> None:
    path = str(selected)
    item = items.get(path)
    if item is None:
        raise ToolError("Worktree changed; refresh")
    if item.current:
        raise ToolError("Current worktree")
    if item.primary:
        raise ToolError("Primary worktree")
    if item.locked:
        raise ToolError("Worktree is locked")
    if item.dirty:
        raise ToolError("Worktree has changes")
    if item.prunable:
        raise ToolError("Worktree path is missing")
    git("worktree", "remove", "--", item.path)


RowsSnapshot = tuple[list[dict[str, object]], dict[str, object]]


def run_picker(mode: str) -> int:
    rows_for: Callable[[], RowsSnapshot]
    rows_for = branch_rows if mode == "branches" else worktree_rows
    rows, items = rows_for()
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
        nonlocal rows, items
        rows, items = rows_for()
        return send({"rows": rows})

    if not send(picker_request(mode, rows)):
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
            if event.get("cancelled"):
                terminal_event = True
                break
            selected = event.get("selected")
            action = event.get("action")
            try:
                if action == "refresh":
                    if not refresh():
                        break
                    continue
                if action == "create" and event.get("input") is not None:
                    if mode == "branches":
                        create_branch(event["input"])
                    else:
                        create_worktree(event["input"], items)
                    if not refresh():
                        break
                    continue
                if action == "delete" and selected is not None:
                    delete_branch(selected, items)
                    if not refresh():
                        break
                    continue
                if action == "remove" and selected is not None:
                    remove_worktree(selected, items)
                    if not refresh():
                        break
                    continue
                if selected is not None:
                    terminal_event = True
                    if mode == "branches":
                        ref = str(selected)
                        branch = items.get(ref)
                        if not isinstance(branch, Branch):
                            raise ToolError("Branch changed; refresh")
                        git("switch", "--", branch.name)
                    else:
                        path = str(selected)
                        item = items.get(path)
                        if not isinstance(item, Worktree):
                            raise ToolError("Worktree changed; refresh")
                        subprocess.run(
                            [ROZI, "new-pane", "--cwd", item.path, "--focus"],
                            check=True,
                        )
                    break
            except (ToolError, subprocess.CalledProcessError, OSError) as error:
                if isinstance(error, ToolError):
                    message = str(error)
                elif isinstance(error, subprocess.CalledProcessError):
                    message = "Rozi could not open the worktree"
                else:
                    message = "Rozi command unavailable"
                notify_error(message)
                try:
                    if not refresh():
                        break
                except ToolError as refresh_error:
                    notify_error(str(refresh_error))
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
    if os.environ.get("ROZI_EXTENSION") != "git-tools":
        print("git-tools must be launched by Rozi", file=sys.stderr)
        return 2
    mode = sys.argv[1] if len(sys.argv) > 1 else "branches"
    if mode not in {"branches", "worktrees"}:
        print(f"unknown mode: {mode}", file=sys.stderr)
        return 2
    if shutil.which("git") is None:
        notify_error("Git not found")
        return 0
    try:
        repository_root()
        return run_picker(mode)
    except ToolError as error:
        notify_error(str(error))
        return 0


if __name__ == "__main__":
    raise SystemExit(main())
