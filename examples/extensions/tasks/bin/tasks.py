#!/usr/bin/env python3
"""Run project tasks discovered from just, make, and package.json.

Uses only the Python standard library and the public `rozi` CLI. Nothing here imports Rozi source
or assumes anything the extension documentation does not state.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

ROZI = os.environ.get("ROZI_BIN") or "rozi"

# Files that mark the top of a project. `.git` is last: a repository can contain several task files,
# and the nearest one is the one the pane is working in.
ROOT_MARKERS = (
    "justfile",
    "Justfile",
    ".justfile",
    "Makefile",
    "makefile",
    "GNUmakefile",
    "package.json",
    ".git",
)

# A make target line, excluding variable assignments (`x := y`) and pattern/special targets.
MAKE_TARGET = re.compile(r"^([A-Za-z0-9][A-Za-z0-9._-]*)\s*:(?!=)")
# A just recipe line. Parameters and dependencies are allowed after the name.
JUST_RECIPE = re.compile(r"^([A-Za-z0-9][A-Za-z0-9_-]*)(\s+[^:]*)?:(?!=)")

STATE_FILE = "last-task.json"


@dataclass(frozen=True)
class Task:
    source: str
    name: str
    command: list[str]
    detail: str

    @property
    def id(self) -> str:
        return f"{self.source}:{self.name}"


def settings() -> dict:
    """Merged settings, as Rozi hands them over.

    Every key is read through a default because a user's wrong-typed value is dropped by Rozi
    before it reaches here, and an older Rozi may not send the variable at all.
    """
    try:
        raw = json.loads(os.environ.get("ROZI_EXTENSION_CONFIG") or "{}")
    except json.JSONDecodeError:
        raw = {}
    if not isinstance(raw, dict):
        raw = {}
    return {
        "sources": raw.get("sources") or ["just", "make", "npm"],
        "pane": raw.get("pane") or "focus",
        "keep_open": raw.get("keep_open", True),
        "workspace": raw.get("workspace", 0),
    }


def project_root(start: Path) -> Path:
    for directory in (start, *start.parents):
        if any((directory / marker).exists() for marker in ROOT_MARKERS):
            return directory
    return start


def read_lines(path: Path) -> list[str]:
    try:
        return path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return []


def just_tasks(root: Path) -> list[Task]:
    justfile = next(
        (root / name for name in ("justfile", "Justfile", ".justfile") if (root / name).is_file()),
        None,
    )
    if justfile is None:
        return []
    names: list[str] = []
    for line in read_lines(justfile):
        if line.startswith((" ", "\t", "#", "@")) or not line.strip():
            continue
        match = JUST_RECIPE.match(line)
        if match and match.group(1) not in names:
            names.append(match.group(1))
    return [
        Task("just", name, ["just", name], f"just {name}")
        for name in names
    ]


def make_tasks(root: Path) -> list[Task]:
    makefile = next(
        (root / name for name in ("Makefile", "makefile", "GNUmakefile") if (root / name).is_file()),
        None,
    )
    if makefile is None:
        return []
    names: list[str] = []
    for line in read_lines(makefile):
        if line.startswith((" ", "\t", "#", ".")):
            continue
        match = MAKE_TARGET.match(line)
        if match and match.group(1) not in names:
            names.append(match.group(1))
    return [Task("make", name, ["make", name], f"make {name}") for name in names]


def npm_tasks(root: Path) -> list[Task]:
    manifest = root / "package.json"
    if not manifest.is_file():
        return []
    try:
        scripts = json.loads(manifest.read_text(encoding="utf-8")).get("scripts") or {}
    except (OSError, json.JSONDecodeError):
        return []
    runner = "pnpm" if (root / "pnpm-lock.yaml").is_file() else (
        "yarn" if (root / "yarn.lock").is_file() else "npm"
    )
    argv = (lambda name: [runner, "run", name]) if runner != "yarn" else (
        lambda name: ["yarn", name]
    )
    return [
        Task("npm", name, argv(name), str(body)[:80])
        for name, body in scripts.items()
        if isinstance(name, str) and name
    ]


DISCOVERY = {"just": just_tasks, "make": make_tasks, "npm": npm_tasks}


def discover(root: Path, sources: list[str]) -> list[Task]:
    tasks: list[Task] = []
    for source in sources:
        finder = DISCOVERY.get(source)
        if finder is not None:
            tasks.extend(finder(root))
    return tasks


def state_path() -> Path | None:
    """Per-project scratch file, kept out of the installed extension directory."""
    base = os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")
    directory = Path(base) / "rozi-tasks"
    try:
        directory.mkdir(parents=True, exist_ok=True)
    except OSError:
        return None
    return directory / STATE_FILE


def remember(task: Task, root: Path) -> None:
    path = state_path()
    if path is None:
        return
    try:
        path.write_text(
            json.dumps({"root": str(root), "id": task.id, "command": task.command}),
            encoding="utf-8",
        )
    except OSError:
        pass


def recall() -> dict | None:
    path = state_path()
    if path is None or not path.is_file():
        return None
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return None


def rozi(args: list[str], stdin: str | None = None) -> subprocess.CompletedProcess:
    return subprocess.run(
        [ROZI, *args],
        input=stdin,
        capture_output=True,
        text=True,
        check=False,
    )


def notify(message: str, level: str = "error") -> None:
    """Report to the user, and take over reporting from Rozi.

    Rozi turns a non-zero exit from an extension command into an error toast of its own. Anything
    reported here is therefore followed by `return 0`: two toasts for one problem, the second less
    specific than the first, is worse than one.
    """
    rozi(["notify", message, "--title", "Tasks", "--level", level])


def run_task(task: Task, root: Path, config: dict) -> int:
    args = ["split", "--cwd", str(root), "--title", task.name]
    if config["pane"] == "focus":
        args.append("--focus")
    if config["keep_open"]:
        args.append("--keep-open")
    workspace = config["workspace"]
    if isinstance(workspace, int) and 1 <= workspace <= 9:
        args += ["--workspace", str(workspace)]
    args.append("--argv")
    args += task.command
    result = rozi(args)
    if result.returncode != 0:
        notify(f"could not start {task.name}: {result.stderr.strip() or 'unknown error'}")
        return 0
    remember(task, root)
    return 0


def rows(tasks: list[Task], active: str | None) -> list[dict]:
    labels = {"just": "just", "make": "make", "npm": "package.json"}
    return [
        {
            "id": task.id,
            "label": task.name,
            "description": task.detail,
            "group": labels.get(task.source, task.source),
            "active": task.id == active,
        }
        for task in tasks
    ]


def command_pick(root: Path, config: dict) -> int:
    tasks = discover(root, config["sources"])
    if not tasks:
        notify(f"no just, make, or package.json tasks under {root}", level="info")
        return 0
    by_id = {task.id: task for task in tasks}
    last = recall() or {}
    active = last.get("id") if last.get("root") == str(root) else None

    opening = {
        "title": f"Tasks · {root.name}",
        "placeholder": "Filter tasks…",
        "actions": [{"id": "refresh", "key": "ctrl-r", "label": "refresh"}],
        "rows": rows(tasks, active),
    }
    picker = subprocess.Popen(
        [ROZI, "pick", "--json"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    assert picker.stdin is not None and picker.stdout is not None
    picker.stdin.write(json.dumps(opening) + "\n")
    picker.stdin.flush()

    try:
        for line in picker.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                continue
            if message.get("cancelled"):
                return 0
            if message.get("action") == "refresh":
                tasks = discover(root, config["sources"])
                by_id = {task.id: task for task in tasks}
                picker.stdin.write(json.dumps({"rows": rows(tasks, active)}) + "\n")
                picker.stdin.flush()
                continue
            selected = message.get("selected")
            if selected is None:
                continue
            task = by_id.get(selected)
            if task is None:
                # The row set was replaced under the selection; nothing to run and nothing worth
                # telling the user about.
                return 0
            return run_task(task, root, config)
    finally:
        picker.stdin.close()
        picker.wait(timeout=5)
    # The stream ended without a selection. That is a cancel when the picker ran, and a failure
    # when it never opened - no live UI, or another overlay already holding the screen. The failure
    # is the one case nothing here reported, so it is left to Rozi's own exit-status toast.
    return 0 if picker.returncode == 0 else 1


def command_repeat(root: Path, config: dict) -> int:
    last = recall()
    if not last or not last.get("command"):
        notify("no task has been run yet", level="info")
        return 0
    remembered = Path(last.get("root") or root)
    task = Task("last", Path(last["command"][-1]).name, list(last["command"]), " ".join(last["command"]))
    return run_task(task, remembered, config)


def command_list(root: Path, config: dict) -> int:
    """One line per task for a sidebar command tab, with `## ` marking each section header.

    Rows carry the whole command line rather than the task name, because the tab's `on_click` can
    only `send` the clicked row as literal text - a bare name would type `build` into the shell and
    do nothing. No trailing newline is sent, so a stray click stages the command and leaves pressing
    Enter to the user.

    Anything that is not a task is printed as a header, which is the only row a command tab can make
    inert. A plain line here would be clickable, and clicking it would type it into the terminal.
    """
    tasks = discover(root, config["sources"])
    if not tasks:
        print(f"## no tasks under {root.name}")
        return 0
    labels = {"just": "just", "make": "make", "npm": "package.json"}
    current = None
    for task in tasks:
        if task.source != current:
            print(f"## {labels.get(task.source, task.source)}")
            current = task.source
        print(" ".join(task.command))
    return 0


COMMANDS = {"pick": command_pick, "repeat": command_repeat, "list": command_list}


def main(argv: list[str]) -> int:
    if len(argv) != 2 or argv[1] not in COMMANDS:
        print(f"usage: tasks.py {{{'|'.join(COMMANDS)}}}", file=sys.stderr)
        return 2
    config = settings()
    root = project_root(Path.cwd())
    return COMMANDS[argv[1]](root, config)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
