from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "bin" / "claude_sessions.py"


def load_script():
    spec = importlib.util.spec_from_file_location("claude_sessions_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


claude_sessions = load_script()

# The shape `claude agents --json` prints: one entry per live session, interactive or background.
CLAUDE_OUTPUT = json.dumps(
    [
        {
            "pid": 9408,
            "id": "ec65255f",
            "cwd": "/home/x/rozi/.claude/worktrees/remote-install",
            "kind": "background",
            "startedAt": 2,
            "sessionId": "ec65255f-ce6d",
            "name": "remote install prompt ux",
            "status": "idle",
            "state": "done",
        },
        {
            "pid": 45936,
            "id": "da3eba77",
            "cwd": "/home/x/rozi",
            "kind": "background",
            "startedAt": 1,
            "sessionId": "da3eba77-194c",
            "name": "worktree unlock",
            "status": "busy",
            "state": "working",
        },
        {
            "pid": 139999,
            "id": "739c363f",
            "cwd": "/home/x/tui-lipan",
            "kind": "background",
            "startedAt": 3,
            "sessionId": "739c363f-6418",
            "status": "idle",
            "state": "needs_input",
        },
        {
            "pid": 700,
            "id": "11111111",
            "cwd": "/home/x/rozi",
            "kind": "interactive",
            "startedAt": 0,
            "sessionId": "11111111-0000",
            "status": "busy",
            "state": "working",
        },
    ]
)


def panes_output(*panes: dict[str, object]) -> str:
    return json.dumps({"ok": True, "data": list(panes)})


class SessionTests(unittest.TestCase):
    def test_states_map_onto_rozis_vocabulary(self) -> None:
        def state(value: str, status: str = "idle") -> tuple[str, str | None]:
            return claude_sessions.Session.from_wire(
                {"sessionId": "s", "state": value, "status": status}
            ).row_state()

        self.assertEqual(state("working"), ("working", None))
        self.assertEqual(state("done"), ("done", None))
        self.assertEqual(state("needs_input"), ("blocked", None))
        self.assertEqual(state("stopped"), ("idle", "Stopped"))
        self.assertEqual(state("failed"), ("idle", "Session failed"))
        # An unknown word must not reach Rozi, which would read it as a live run.
        self.assertEqual(state("pondering", "busy"), ("working", None))
        self.assertEqual(state("pondering"), ("idle", None))

    def test_a_row_carries_the_directory_its_session_works_in(self) -> None:
        sessions = claude_sessions.parse_sessions(CLAUDE_OUTPUT)
        self.assertEqual(
            sessions[0].row(),
            {
                "id": "ec65255f-ce6d",
                "title": "remote install prompt ux",
                "status": "done",
                "active": False,
                "cwd": "/home/x/rozi/.claude/worktrees/remote-install",
            },
        )
        # Unnamed sessions keep an empty title; Rozi never shows an opaque id instead.
        self.assertEqual(sessions[2].row()["title"], "")

    def test_malformed_entries_are_skipped_and_garbage_is_an_error(self) -> None:
        self.assertEqual(
            claude_sessions.parse_sessions('[{"pid": 1}, 7, {"id": "a"}]')[0].session_id,
            "a",
        )
        with self.assertRaises(claude_sessions.SessionsError):
            claude_sessions.parse_sessions("not json")
        with self.assertRaises(claude_sessions.SessionsError):
            claude_sessions.parse_sessions('{"sessions": []}')


class HostPaneTests(unittest.TestCase):
    def setUp(self) -> None:
        self.sessions = claude_sessions.parse_sessions(CLAUDE_OUTPUT)

    def hosts(self, *panes: dict[str, object]) -> list[int]:
        parsed = claude_sessions.parse_panes(panes_output(*panes))
        return [pane.pane_id for pane in claude_sessions.host_panes(parsed, self.sessions)]

    def test_only_a_claude_pane_that_is_not_itself_a_session_hosts_rows(self) -> None:
        self.assertEqual(
            self.hosts(
                # Claude's session view: Claude in front, but not one of its sessions.
                {"id": 1, "foreground_program": "claude", "foreground_pid": 500},
                # An ordinary interactive session, listed by Claude under its own pid.
                {"id": 2, "foreground_program": "claude", "foreground_pid": 700},
                # Not Claude at all.
                {"id": 3, "foreground_program": "nvim", "foreground_pid": 900},
                # Claude, but no pid to compare: left to Rozi's own detection.
                {"id": 4, "agent": "claude"},
                # Recognized by detection rather than by executable name.
                {"id": 5, "agent": "claude", "foreground_program": "node", "foreground_pid": 600},
            ),
            [1, 5],
        )

    def test_a_failed_listing_is_an_error(self) -> None:
        with self.assertRaises(claude_sessions.SessionsError):
            claude_sessions.parse_panes('{"ok": false, "error": "no UI"}')


class PaneRowTests(unittest.TestCase):
    def setUp(self) -> None:
        self.sessions = claude_sessions.parse_sessions(CLAUDE_OUTPUT)
        self.pane = claude_sessions.Pane(
            pane_id=1, cwd="/home/x/rozi", program="claude", agent=None, foreground_pid=500
        )

    def test_every_background_session_is_listed_oldest_first(self) -> None:
        rows = claude_sessions.pane_rows(self.pane, self.sessions, "all")
        self.assertEqual(
            [row["id"] for row in rows],
            ["da3eba77-194c", "ec65255f-ce6d", "739c363f-6418"],
        )

    def test_cwd_scope_keeps_sessions_started_under_the_pane(self) -> None:
        rows = claude_sessions.pane_rows(self.pane, self.sessions, "cwd")
        self.assertEqual(
            [row["id"] for row in rows], ["da3eba77-194c", "ec65255f-ce6d"]
        )
        # A sibling directory sharing a prefix is not "under" the pane.
        self.assertFalse(claude_sessions.is_under("/home/x/rozi-old", "/home/x/rozi"))


class SettingsTests(unittest.TestCase):
    def test_settings_fall_back_to_defaults(self) -> None:
        settings = claude_sessions.Settings.from_environment(
            {"ROZI_EXTENSION_CONFIG": '{"poll_seconds": 0, "scope": "bogus", "claude": " "}'}
        )
        self.assertEqual(settings, claude_sessions.Settings(poll_seconds=0.5))
        self.assertEqual(
            claude_sessions.Settings.from_environment({}), claude_sessions.Settings()
        )


if __name__ == "__main__":
    unittest.main()
