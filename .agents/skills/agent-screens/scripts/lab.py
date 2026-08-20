#!/usr/bin/env python3
"""Set up panes for a screen-capture session. It launches agents; it never drives them.

Everything past `launch` is a person's job: answering trust prompts, typing the prompt for a
checkpoint, deciding an agent is misbehaving. A script cannot tell an approval dialog from a
question chooser in a tool it has never seen, and the attempts to make it do so were what kept
answering dialogs that should have been captured unanswered.

    lab.py list                        # installed agents, and what each still needs
    lab.py launch pi codex grok        # one pane each, six to a workspace
    lab.py launch --all-ready          # every installed agent still missing a screen
    lab.py launch --workspace 4 pi     # start numbering somewhere else

Requires a running Rozi. Uses $ROZI_SOCKET, or --socket PATH.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gaps  # noqa: E402

REPO = gaps.REPO

# One directory, reused across runs, rather than a fresh temp dir each time. Agents ask whether they
# trust a directory the first time they see it, and that question is a screen worth capturing - so
# the directory has to be one a person can approve once and be done with, not one that resets the
# prompt on every run.
DEFAULT_WORKDIR = Path(
    os.environ.get("XDG_CACHE_HOME", Path.home() / ".cache")
) / "rozi-agent-lab"

README = """# Sample project

A minimal project, here to give an agent a short, harmless file to read.
"""

# Six panes on a normal display land around 88x39, which is where every usable capture in this
# corpus came from. Narrower panes wrap the status chrome agents draw at the bottom, and a wrapped
# footer is a screen detection reads differently from the one a person is looking at.
PER_WORKSPACE = 6


def rozi() -> str:
    local = REPO / "target/debug/rozi"
    return str(local) if local.exists() else "rozi"


def layout_cycle() -> list[str]:
    """`LayoutKind::all()`, which is the order `toggle-layout` rotates through.

    Read out of the source rather than copied, because a reorder there would otherwise leave this
    script confidently sending the wrong number of toggles.
    """
    text = (REPO / "src/state/layout.rs").read_text(encoding="utf-8")
    match = re.search(r"pub fn all\(\) -> &'static \[LayoutKind\] \{\s*&\[(.*?)\]", text, re.S)
    if not match:
        raise SystemExit("src/state/layout.rs has no `LayoutKind::all()` to read the cycle from")
    return [name.lower() for name in re.findall(r"Self::(\w+)", match.group(1))]


def configured_layout() -> str:
    """`[layout] default` from the config this machine actually loads, or dwindle."""
    path = os.environ.get("ROZI_CONFIG")
    config = Path(path) if path else Path(
        os.environ.get("XDG_CONFIG_HOME", Path.home() / ".config")
    ) / "rozi/config.toml"
    if not config.exists():
        return "dwindle"
    try:
        data = tomllib.loads(config.read_text(encoding="utf-8"))
    except (OSError, tomllib.TOMLDecodeError):
        return "dwindle"
    value = data.get("layout", {}).get("default", "dwindle")
    return str(value).strip().lower()


def toggles_to_grid() -> tuple[int, str]:
    """How many `toggle-layout` runs turn a freshly created workspace into a grid."""
    cycle = layout_cycle()
    start = configured_layout()
    if start not in cycle or "grid" not in cycle:
        raise SystemExit(f"cannot get from `{start}` to grid through {cycle}")
    return (cycle.index("grid") - cycle.index(start)) % len(cycle), start


def control(socket: str | None, *args: str) -> dict:
    env = dict(os.environ)
    if socket:
        env["ROZI_SOCKET"] = socket
    done = subprocess.run(
        [rozi(), *args], capture_output=True, text=True, env=env, check=False
    )
    if not done.stdout.strip():
        raise SystemExit(
            f"no reply from rozi {' '.join(args)}\n"
            f"{done.stderr.strip() or 'is a session running, and is ROZI_SOCKET set?'}"
        )
    reply = json.loads(done.stdout)
    if not reply.get("ok", False):
        raise SystemExit(f"rozi {' '.join(args)} failed: {reply.get('error', reply)}")
    return reply


def prepare_workdir(path: Path) -> None:
    path.mkdir(parents=True, exist_ok=True)
    readme = path / "README.md"
    if not readme.exists():
        readme.write_text(README, encoding="utf-8")


def cmd_list(args: argparse.Namespace) -> int:
    gaps.print_report(gaps.report(), ready_only=True)
    return 0


def cmd_launch(args: argparse.Namespace) -> int:
    data = gaps.report()
    by_id = {row["id"]: row for row in data["agents"]}

    if args.all_ready:
        wanted = [
            row["id"]
            for row in data["agents"]
            if row["program"] and (row["missing"] or row["missing_blocked"])
        ]
    else:
        wanted = args.agents

    if not wanted:
        raise SystemExit("name at least one agent, or pass --all-ready")

    chosen = []
    for agent_id in wanted:
        row = by_id.get(agent_id)
        if row is None:
            print(f"  skip {agent_id}: no such agent in builtin.toml", file=sys.stderr)
            continue
        if not row["program"]:
            print(f"  skip {agent_id}: not installed here", file=sys.stderr)
            continue
        chosen.append(row)

    if not chosen:
        raise SystemExit("nothing to launch")

    workdir = Path(args.cwd) if args.cwd else DEFAULT_WORKDIR
    prepare_workdir(workdir)

    workspaces: dict[int, list[str]] = {}
    for index, row in enumerate(chosen):
        workspace = args.workspace + index // PER_WORKSPACE
        if workspace > 9:
            print(
                f"  skip {row['id']}: would need workspace {workspace}; there are only 9",
                file=sys.stderr,
            )
            continue
        workspaces.setdefault(workspace, []).append(row["program"])
        if args.dry_run:
            continue
        control(
            args.socket,
            "new-pane",
            "--workspace",
            str(workspace),
            "--cwd",
            str(workdir),
            "--title",
            row["id"],
            row["program"],
        )

    verb = "would launch" if args.dry_run else "launched"
    print(f"{verb} in {workdir}:")
    for workspace in sorted(workspaces):
        print(f"  workspace {workspace}: {', '.join(workspaces[workspace])}")

    if args.no_layout:
        print("\nLayout left alone (--no-layout). Grid keeps the panes equal size: `prefix m`.")
    else:
        # There is no control command that *sets* a layout, only one that steps to the next, and no
        # way to read which layout a workspace is on. So this counts from the configured default,
        # which is where a workspace created moments ago still is - unless somebody has already
        # changed it, which is what --no-layout is for.
        steps, start = toggles_to_grid()
        for workspace in sorted(workspaces):
            if args.dry_run:
                continue
            control(args.socket, "switch-workspace", str(workspace))
            for _ in range(steps):
                control(args.socket, "run-action", "toggle-layout")
        target = min(workspaces) if workspaces else args.workspace
        if not args.dry_run:
            control(args.socket, "switch-workspace", str(target))
        print(
            f"\n{'would set' if args.dry_run else 'set'} each workspace to grid: "
            f"{steps} x toggle-layout from `{start}`"
            + ("" if args.dry_run else f", now on workspace {target}")
        )
        print("  If a workspace was not on the default layout, it is now somewhere else in the")
        print("  cycle - `prefix shift-m` picks grid outright.")

    print(
        "\nNow, by hand:\n"
        "  1. Answer any trust prompt you *want* answered - and say so, because an unanswered\n"
        "     one is itself a screen worth capturing.\n"
        "  2. Tell the agent the panes are up."
    )
    return 0


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--socket", help="control socket path (default $ROZI_SOCKET)")
    sub = parser.add_subparsers(dest="command", required=True)

    listing = sub.add_parser("list", help="installed agents and what each still needs")
    listing.set_defaults(func=cmd_list)

    launch = sub.add_parser("launch", help="spawn one pane per agent")
    launch.add_argument("agents", nargs="*", help="agent ids from builtin.toml")
    launch.add_argument(
        "--all-ready",
        action="store_true",
        help="every installed agent still missing a screen",
    )
    launch.add_argument(
        "--workspace", type=int, default=2, help="first workspace to fill (default 2)"
    )
    launch.add_argument("--cwd", help=f"working directory (default {DEFAULT_WORKDIR})")
    launch.add_argument(
        "--no-layout",
        action="store_true",
        help="do not switch the workspaces to grid (they are not on the default layout)",
    )
    launch.add_argument(
        "--dry-run", action="store_true", help="print the plan without spawning"
    )
    launch.set_defaults(func=cmd_launch)

    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())
