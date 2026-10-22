"""Tests for the agent-board extension.

Run from the extension directory:

    python -m unittest discover -s tests
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "bin"))

import board  # noqa: E402
import demo_agent  # noqa: E402

PANES = [
    {"id": 1, "title": "shell", "agent": None, "agent_state": None, "reported_status": None},
    {"id": 2, "title": "fix tests", "agent": "claude", "agent_state": "working",
     "reported_status": None},
    {"id": 3, "title": "", "agent": "agent-board.demo", "agent_state": "idle",
     "reported_status": "blocked"},
]


class BoardTest(unittest.TestCase):
    def test_only_agent_panes_get_rows(self) -> None:
        rows = board.tab_snapshot(board.agent_panes(PANES))["rows"]
        self.assertEqual(
            [(row["id"], row["label"], row["meta"], row["tone"]) for row in rows],
            [("2", "fix tests", "working", "working"), ("3", "agent-board.demo", "blocked", "blocked")],
        )

    def test_the_activity_row_leads_with_what_needs_you(self) -> None:
        (row,) = board.activity_snapshot(board.agent_panes(PANES))["rows"]
        self.assertEqual(row["status"], "blocked")
        self.assertEqual(row["reason"], "1 waiting for you, 1 working, 2 in all")
        self.assertEqual(board.activity_snapshot([]), {"rows": []})

    def test_activating_a_row_focuses_its_pane(self) -> None:
        self.assertEqual(board.handle({"activate": "3"}), [board.ROZI, "focus", "3"])
        self.assertIsNone(board.handle({"visible": True}))


class DemoAgentTest(unittest.TestCase):
    def test_the_manifest_rules_match_what_the_demo_agent_prints(self) -> None:
        try:
            import tomllib
        except ModuleNotFoundError:
            self.skipTest("tomllib needs Python 3.11")
        manifest = tomllib.loads((ROOT / "extension.toml").read_text(encoding="utf-8"))
        (agent,) = manifest["agents"]
        self.assertTrue(any(path in "bin/demo_agent.py" for path in agent["match"]["paths"]))
        screens = {"working": demo_agent.WORKING, "blocked": demo_agent.ASKING}
        for rule in agent["states"]:
            self.assertTrue(
                any(text in screens[rule["state"]] for text in rule["screen"]["any_of"]),
                rule,
            )


if __name__ == "__main__":
    unittest.main()
