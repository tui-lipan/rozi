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
    {"id": 3, "title": "", "agent": "agent-board.demo", "agent_state": "blocked",
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

    def test_effective_state_is_not_overridden_by_generic_reports(self) -> None:
        # list-panes already includes integration reports and detection in agent_state.
        for state in ("blocked", "error", "working", "done", "idle"):
            for reported in ("working", "blocked", "done", "idle", "error", "compiling", None):
                with self.subTest(state=state, reported=reported):
                    pane = {**PANES[1], "agent_state": state, "reported_status": reported}
                    (row,) = board.tab_snapshot([pane])["rows"]
                    self.assertEqual(board.state_of(pane), state)
                    self.assertEqual((row["meta"], row["tone"]), (state, board.TONES[state]))
        self.assertEqual(board.state_of({}), "idle")

    def test_activity_preserves_failures_and_uses_core_urgency(self) -> None:
        cases = (
            (["error"], "error"),
            (["working", "error"], "error"),
            (["error", "blocked"], "blocked"),
            (["idle", "done"], "done"),
            (["done", "working"], "working"),
            (["done"], "done"),
            (["idle"], "idle"),
        )
        for states, expected in cases:
            with self.subTest(states=states):
                agents = [{**PANES[1], "agent_state": state} for state in states]
                (row,) = board.activity_snapshot(agents)["rows"]
                self.assertEqual(row["status"], expected)
                (reversed_row,) = board.activity_snapshot(list(reversed(agents)))["rows"]
                self.assertEqual(reversed_row["status"], expected)

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
        (service,) = manifest["services"]
        self.assertEqual(service["placement"], "active-session")
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
