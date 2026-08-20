#!/usr/bin/env python3
"""What the agent-screen corpus has, what it is missing, and what could be captured today.

Everything printed here is derived from three files that already exist - the agent manifest, the
fixture corpus, and the ledgers in `fixtures.rs` - so the report cannot drift the way a
hand-maintained checklist does. Nothing is written; this only reads.

    gaps.py                 # the full report
    gaps.py --ready         # only agents installed here with something still missing
    gaps.py --json          # the same data, for a script
"""

from __future__ import annotations

import argparse
import json
import re
import shutil
import sys
import tomllib
from pathlib import Path

REPO = Path(__file__).resolve().parents[4]
BUILTIN = REPO / "src/agent_detection/builtin.toml"
CORPUS = REPO / "tests/fixtures/agents"
LEDGERS = REPO / "src/agent_detection/fixtures.rs"

# The three states detection can report, in the order a person would capture them.
STATES = ("idle", "working", "blocked")

# Blocked is not one screen. An agent can stop for a directory it has not seen, for permission to
# do something, or to ask a question, and those look nothing alike - a corpus with one of them is
# not covered for the other two. Case names carry which is which.
BLOCKED_KINDS = {
    "trust": ("trust",),
    "approval": ("approval", "permission"),
    "question": ("question", "chooser", "asking"),
}

# A manifest name that means something else on this machine. `match.names` lists what to *recognize*
# a running process by, not what to launch: `cursor` is the GUI editor; the agent CLI is
# `cursor-agent`.
LAUNCH_OVERRIDE = {"cursor": "cursor-agent"}


def agents() -> list[dict]:
    """Every agent Rozi ships a definition for, in manifest order."""
    manifest = tomllib.loads(BUILTIN.read_text(encoding="utf-8"))
    out = []
    for entry in manifest.get("agents", []):
        names = entry.get("match", {}).get("names", [])
        out.append(
            {
                "id": entry["id"],
                "label": entry.get("label", entry["id"]),
                "names": names,
                "rules": len(entry.get("states", [])),
                "base": entry.get("base", True),
            }
        )
    return out


def ledger(name: str) -> list[str]:
    """One `const NAME: &[&str] = &[...]` list from `fixtures.rs`, comments stripped."""
    text = LEDGERS.read_text(encoding="utf-8")
    match = re.search(rf"const {name}: &\[&str\] = &\[(.*?)\];", text, re.S)
    if not match:
        # Silently returning nothing would report every excused agent as an unadmitted gap, which
        # reads as a much bigger problem than a renamed constant.
        raise SystemExit(
            f"{LEDGERS.name} has no `const {name}`. It was renamed or restructured; "
            "this report is derived from it and cannot guess."
        )
    body = "\n".join(
        line for line in match.group(1).splitlines() if not line.strip().startswith("//")
    )
    return re.findall(r'"([^"]+)"', body)


def fixtures() -> dict[str, dict]:
    """Each agent's captured screens, keyed by agent id."""
    found = {}
    for path in sorted(CORPUS.glob("*.toml")):
        data = tomllib.loads(path.read_text(encoding="utf-8"))
        found[path.stem] = {
            "path": path,
            "source": data.get("source", "?"),
            "captured_at": data.get("captured_at", "?"),
            "cases": [
                {"name": case.get("name", "?"), "state": case.get("state", "?")}
                for case in data.get("case", [])
            ],
        }
    return found


def blocked_kinds(cases: list[dict]) -> set[str]:
    kinds = set()
    for case in cases:
        if case["state"] != "blocked":
            continue
        name = case["name"].lower()
        for kind, needles in BLOCKED_KINDS.items():
            if any(needle in name for needle in needles):
                kinds.add(kind)
                break
        else:
            kinds.add("other")
    return kinds


def installed(names: list[str], agent_id: str) -> str | None:
    """The executable to launch for this agent, if one is on PATH."""
    override = LAUNCH_OVERRIDE.get(agent_id)
    if override and shutil.which(override):
        return override
    for name in names:
        if shutil.which(name):
            return name
    return None


def report() -> dict:
    awaiting = set(ledger("AWAITING_EVIDENCE"))
    in_unit_tests = set(ledger("EVIDENCE_IN_UNIT_TESTS"))
    corpus = fixtures()
    rows = []
    for agent in agents():
        entry = corpus.get(agent["id"])
        cases = entry["cases"] if entry else []
        states = {case["state"] for case in cases}
        rows.append(
            {
                "id": agent["id"],
                "label": agent["label"],
                "rules": agent["rules"],
                "base": agent["base"],
                "program": installed(agent["names"], agent["id"]),
                "has_fixture": entry is not None,
                "captured_at": entry["captured_at"] if entry else None,
                "cases": cases,
                "have": sorted(states & set(STATES)),
                # `blocked` is reported by kind instead, so it is never also listed bare.
                "missing": [
                    state for state in STATES if state not in states and state != "blocked"
                ],
                "blocked_kinds": sorted(blocked_kinds(cases)),
                "missing_blocked": sorted(set(BLOCKED_KINDS) - blocked_kinds(cases)),
                "awaiting_evidence": agent["id"] in awaiting,
                "evidence_in_unit_tests": agent["id"] in in_unit_tests,
            }
        )
    return {"agents": rows}


def mark(present: bool) -> str:
    return "yes" if present else " - "


def print_report(data: dict, ready_only: bool) -> None:
    rows = data["agents"]
    if ready_only:
        rows = [r for r in rows if r["program"] and (r["missing"] or r["missing_blocked"])]
        if not rows:
            print("Every agent installed here has all three states captured.")
            print("Next gaps need a tool that is not installed - see the full report.")
            return
        print("Installed here, and still missing something:\n")
    else:
        print("Agent screen corpus\n")

    width = max(len(r["id"]) for r in rows)
    kind_width = max(24, max(len(",".join(r["blocked_kinds"])) for r in rows) + 2)
    header = (
        f"  {'agent'.ljust(width)}  idle  work  {'blocked'.ljust(kind_width)}launch"
    )
    print(header)
    print("  " + "-" * (len(header) - 2))
    for row in rows:
        have = set(row["have"])
        kinds = ",".join(row["blocked_kinds"]) or "-"
        launch = row["program"] or "(not installed)"
        print(
            f"  {row['id'].ljust(width)}  {mark('idle' in have)}   {mark('working' in have)}   "
            f"{kinds.ljust(kind_width)}{launch}"
        )

    if ready_only:
        print("\nMissing, per agent:")
        for row in rows:
            gaps = list(row["missing"])
            gaps += [f"blocked/{kind}" for kind in row["missing_blocked"]]
            print(f"  {row['id']}: {', '.join(gaps)}")
        return

    no_fixture = [r for r in rows if not r["has_fixture"]]
    if no_fixture:
        print("\nNo screen at all - detected on the shared vocabulary alone, never watched:")
        for row in no_fixture:
            why = []
            if row["evidence_in_unit_tests"]:
                why.append("excused: asserted inline in mod.rs")
            elif row["awaiting_evidence"]:
                why.append("in AWAITING_EVIDENCE")
            else:
                why.append("in NEITHER ledger - `cargo test` should be failing")
            if row["program"]:
                why.append(f"installed as `{row['program']}`")
            print(f"  {row['id']:<16} {'; '.join(why)}")

    ready = [r for r in rows if r["program"] and (r["missing"] or r["missing_blocked"])]
    if ready:
        print("\nCould be captured on this machine right now:")
        for row in ready:
            gaps = list(row["missing"]) + [f"blocked/{k}" for k in row["missing_blocked"]]
            print(f"  {row['id']:<16} needs {', '.join(gaps)}")
    print("\n  gaps.py --ready   for just that list")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="print the raw data")
    parser.add_argument(
        "--ready",
        action="store_true",
        help="only agents installed here that are still missing a screen",
    )
    args = parser.parse_args()
    data = report()
    if args.json:
        json.dump(data, sys.stdout, indent=2, default=str)
        print()
    else:
        print_report(data, args.ready)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
