from __future__ import annotations

import importlib.util
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "bin" / "agent_activity.py"


def load_script():
    spec = importlib.util.spec_from_file_location("agent_activity_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


agent_activity = load_script()


class PaneWireTests(unittest.TestCase):
    def test_wire_values_are_cleaned_and_defaults_are_applied(self) -> None:
        pane = agent_activity.Pane.from_wire(
            {
                "id": "42",
                "title": "  Agent task  ",
                "workspace": "3",
                "status": " running ",
                "reported_status": " working ",
                "status_reason": "  Run checks  ",
            }
        )

        self.assertIsNotNone(pane)
        self.assertEqual(
            (
                pane.pane_id,
                pane.title,
                pane.workspace,
                pane.terminal_status,
                pane.reported_status,
                pane.reason,
            ),
            (42, "Agent task", 3, "running", "working", "Run checks"),
        )
        self.assertTrue(pane.is_live_activity())
        self.assertEqual(
            pane.published_row(focused_pane=42),
            {
                "id": "pane:42",
                "title": "Run checks",
                "status": "working",
                "active": True,
                "reason": "Agent task",
            },
        )

    def test_invalid_or_exited_wire_panes_are_not_live_activity(self) -> None:
        self.assertIsNone(agent_activity.Pane.from_wire({"title": "missing id"}))
        self.assertIsNone(agent_activity.Pane.from_wire({"id": "not-an-integer"}))

        exited = agent_activity.Pane.from_wire(
            {
                "id": 9,
                "title": "",
                "workspace": "invalid",
                "status": "Exited 1",
                "reported_status": "working",
            }
        )
        self.assertIsNotNone(exited)
        self.assertEqual(exited.title, "Pane 9")
        self.assertEqual(exited.workspace, 0)
        self.assertFalse(exited.is_live_activity())


class ActivityClassificationTests(unittest.TestCase):
    @staticmethod
    def pane(pane_id: int, status: str, workspace: int = 1):
        return agent_activity.Pane(
            pane_id=pane_id,
            title=f"Pane {pane_id}",
            workspace=workspace,
            terminal_status="running",
            reported_status=status,
            reason=None,
        )

    def test_picker_rows_group_and_sort_statuses(self) -> None:
        panes = {
            6: self.pane(6, "mystery"),
            5: self.pane(5, "idle"),
            4: self.pane(4, "done"),
            3: self.pane(3, "working", workspace=2),
            2: self.pane(2, "failed"),
            1: self.pane(1, "blocked"),
            7: agent_activity.Pane(
                pane_id=7,
                title="No report",
                workspace=1,
                terminal_status="running",
                reported_status=None,
                reason=None,
            ),
        }

        rows = agent_activity.picker_rows(panes)

        self.assertEqual(
            [row["id"] for row in rows],
            ["pane:1", "pane:2", "pane:3", "pane:4", "pane:5", "pane:6"],
        )
        self.assertEqual(
            [row["group"] for row in rows],
            ["Blocked", "Errors", "Working", "Finished", "Idle", "Other"],
        )
        self.assertEqual(rows[2]["description"], "working · pane 3 · ws 2")

    def test_transition_classification_only_reports_new_terminal_states(self) -> None:
        cases = [
            ("working", "blocked", "blocked"),
            ("blocked", "blocked", None),
            ("working", "failed: timeout", "error"),
            ("error", "failed", None),
            ("working", "completed", "finished"),
            ("working", "idle", "finished"),
            ("idle", "idle", None),
            ("done", "finished", None),
        ]
        for previous, current, expected in cases:
            with self.subTest(previous=previous, current=current):
                self.assertEqual(
                    agent_activity.transition_kind(previous, current),
                    expected,
                )

    def test_row_ids_only_accept_the_pane_wire_namespace(self) -> None:
        self.assertEqual(agent_activity.pane_id_from_row("pane:12"), 12)
        self.assertIsNone(agent_activity.pane_id_from_row("pr:12"))
        self.assertIsNone(agent_activity.pane_id_from_row("pane:not-a-number"))


if __name__ == "__main__":
    unittest.main()
