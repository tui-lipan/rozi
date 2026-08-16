#!/usr/bin/env python3
"""Canonical git-tools extension: no imports from Rozi, only its public CLI."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path


ROZI = os.environ.get("ROZI_BIN", "rozi")


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["git", *args], text=True, capture_output=True, check=check
    )


def notify(message: str, *, error: bool = False) -> None:
    args = [ROZI, "notify", message]
    if error:
        args.extend(["--title", "Git tools", "--level", "error"])
    subprocess.run(args, check=False)


def branch_rows() -> list[dict[str, object]]:
    output = git(
        "for-each-ref",
        "refs/heads/",
        "--sort=-committerdate",
        "--format=%(HEAD)%09%(refname:short)%09%(committerdate:relative)",
    ).stdout
    rows = []
    for line in output.splitlines():
        marker, name, age = line.split("\t", 2)
        current = marker == "*"
        rows.append(
            {
                "id": name,
                "label": name,
                "group": "Current" if current else "Recent",
                "description": f"current · {age}" if current else age,
                "active": current,
            }
        )
    return rows


def worktree_rows() -> list[dict[str, object]]:
    current = str(Path.cwd().resolve())
    rows = []
    records = git("worktree", "list", "--porcelain").stdout.strip().split("\n\n")
    for record in records:
        fields = dict(
            line.split(" ", 1)
            for line in record.splitlines()
            if " " in line
        )
        path = fields.get("worktree")
        if not path:
            continue
        branch = fields.get("branch", "detached").removeprefix("refs/heads/")
        rows.append(
            {
                "id": path,
                "label": Path(path).name,
                "group": "Current" if path == current else "Other worktrees",
                "description": branch,
                "active": path == current,
            }
        )
    return rows


def request(mode: str, rows: list[dict[str, object]]) -> dict[str, object]:
    if mode == "branches":
        actions = [
            {"id": "refresh", "key": "r", "label": "refresh"},
            {
                "id": "new",
                "key": "ctrl-n",
                "label": "new",
                "prompt": "New branch",
            },
            {
                "id": "delete",
                "key": "ctrl-d",
                "label": "delete",
                "confirm": True,
            },
        ]
        title = "Git branches"
    else:
        actions = [{"id": "refresh", "key": "r", "label": "refresh"}]
        title = "Git worktrees"
    return {
        "title": title,
        "placeholder": "Filter…",
        "actions": actions,
        "rows": rows,
    }


def run_picker(mode: str) -> int:
    rows_for = branch_rows if mode == "branches" else worktree_rows
    process = subprocess.Popen(
        [ROZI, "pick", "--json"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert process.stdin is not None and process.stdout is not None

    def send(payload: dict[str, object]) -> None:
        process.stdin.write(json.dumps(payload) + "\n")
        process.stdin.flush()

    send(request(mode, rows_for()))
    for line in process.stdout:
        event = json.loads(line)
        if event.get("cancelled"):
            break
        selected = event.get("selected")
        action = event.get("action")
        try:
            if action == "new" and event.get("input"):
                name = event["input"].strip()
                if name:
                    git("switch", "-c", name)
                    notify(f"created {name}")
                    break
            if action == "delete" and selected:
                git("branch", "-d", selected)
            if action in {"refresh", "delete"}:
                send({"rows": rows_for()})
                continue
            if selected:
                if mode == "branches":
                    git("switch", selected)
                    notify(f"switched to {selected}")
                else:
                    subprocess.run(
                        [ROZI, "new-pane", "--cwd", selected, "--focus"], check=True
                    )
                break
        except subprocess.CalledProcessError as error:
            detail = (error.stderr or str(error)).strip()
            notify(detail, error=True)
            send({"rows": rows_for()})

    process.stdin.close()
    return process.wait()


def main() -> int:
    if os.environ.get("ROZI_EXTENSION") != "git-tools":
        print("git-tools must be launched by Rozi", file=sys.stderr)
        return 2
    if git("rev-parse", "--is-inside-work-tree", check=False).returncode != 0:
        notify("focused pane is not in a Git repository", error=True)
        return 0
    mode = sys.argv[1] if len(sys.argv) > 1 else "branches"
    if mode not in {"branches", "worktrees"}:
        print(f"unknown mode: {mode}", file=sys.stderr)
        return 2
    return run_picker(mode)


if __name__ == "__main__":
    raise SystemExit(main())
