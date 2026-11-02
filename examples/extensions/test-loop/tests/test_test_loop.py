"""Tests for the test-loop extension, with state in a temporary directory.

Run from the extension directory:

    python3 -m unittest discover -s tests
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import time
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bin"))

import test_loop  # noqa: E402
from test_loop import AgentLedger, Loop, Reply, Settings  # noqa: E402

REF = {"incarnation": 1, "pane": {"pane_id": 2, "generation": 2, "session_instance": "i"}}


def passed() -> Reply:
    return Reply(True, {"result": {"ran": True, "passed": True, "exit_code": 0}})


def failed() -> Reply:
    return Reply(
        True, {"result": {"ran": True, "passed": False, "exit_code": 1, "log": "/state/logs/a.log"}}
    )


class FakeRozi:
    """Answers as `rozi` does, and records what the service asked of it."""

    def __init__(self, cwd: str) -> None:
        self.listed: list[dict] | None = []
        self.cwd = cwd
        self.test_replies: list[Reply] = []
        self.test_runs: list[str] = []
        self.prompts: list[str] = []
        self.prompt_reply = Reply(True)
        self.notes: list[tuple[str, str]] = []

    def finish_turn(self, completions: int, state: str = "done") -> None:
        self.listed = [{"ref": REF, "completions": completions, "state": state, "cwd": self.cwd}]

    def agents(self) -> list[dict] | None:
        return self.listed

    def run_tests(self, cwd: str) -> Reply:
        self.test_runs.append(cwd)
        return self.test_replies.pop(0)

    def prompt(self, reference: dict, text: str) -> Reply:
        self.prompts.append(text)
        return self.prompt_reply

    def notify(self, message: str, level: str = "info") -> None:
        self.notes.append((message, level))


class LoopTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name) / "project"
        self.root.mkdir()
        self.state = Path(directory.name) / "state"
        self.rozi = FakeRozi(str(self.root))
        self.loop = self.make_loop()

    def make_loop(self, max_attempts: int = 2) -> Loop:
        settings = Settings([str(self.root)], ["make", "test"], max_attempts)
        return Loop(self.rozi, settings, AgentLedger(self.state / "agents-i.json"))

    def assert_attempts(self, expected: int) -> None:
        (record,) = self.loop.ledger.agents.values()
        self.assertEqual(record["attempts"], expected)

    def test_a_passing_turn_notifies_and_prompts_nobody(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [passed()]
        self.loop.reconcile()
        self.assertEqual(self.rozi.prompts, [])
        self.assertEqual(self.rozi.notes, [("project: tests pass", "info")])

    def test_a_turn_runs_the_tests_only_once(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [passed()]
        self.loop.reconcile()
        self.loop.reconcile()
        self.assertEqual(len(self.rozi.test_runs), 1)

    def test_a_failure_asks_the_agent_with_one_line_that_names_the_log(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [failed()]
        self.loop.reconcile()
        (prompt,) = self.rozi.prompts
        self.assertIn("make test", prompt)
        self.assertIn("/state/logs/a.log", prompt)
        self.assertIn("attempt 1 of 2", prompt)
        self.assertNotIn("\n", prompt)

    def test_it_gives_up_once_and_says_so(self) -> None:
        self.rozi.test_replies = [failed(), failed(), failed(), failed()]
        for completions in (1, 2, 3, 4):
            self.rozi.finish_turn(completions)
            self.loop.reconcile()
        self.assertEqual(len(self.rozi.prompts), 2)
        errors = [note for note in self.rozi.notes if note[1] == "error"]
        self.assertEqual(len(errors), 1)
        self.assertIn("still fail after 2 fix attempts", errors[0][0])

    def test_a_pass_after_giving_up_starts_the_count_again(self) -> None:
        self.rozi.test_replies = [failed(), failed(), failed(), passed(), failed()]
        for completions in (1, 2, 3, 4, 5):
            self.rozi.finish_turn(completions)
            self.loop.reconcile()
        self.assertEqual(len(self.rozi.prompts), 3)
        self.assertIn(("project: tests pass after 2 fix attempts", "info"), self.rozi.notes)

    def test_a_turn_that_ended_during_a_restart_is_still_handled(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [passed(), passed()]
        self.loop.reconcile()
        self.rozi.finish_turn(2)
        self.make_loop().reconcile()
        self.assertEqual(len(self.rozi.test_runs), 2)

    def test_a_restart_does_not_repeat_a_handled_turn(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [failed()]
        self.loop.reconcile()
        self.make_loop().reconcile()
        self.assertEqual(len(self.rozi.test_runs), 1)
        self.assertEqual(len(self.rozi.prompts), 1)

    def test_a_restart_keeps_the_attempts_it_already_spent(self) -> None:
        self.rozi.test_replies = [failed(), failed()]
        self.rozi.finish_turn(1)
        self.loop.reconcile()
        self.rozi.finish_turn(2)
        self.make_loop().reconcile()
        self.assertIn("attempt 2 of 2", self.rozi.prompts[1])

    def test_an_agent_that_is_not_stopped_waits(self) -> None:
        for state in ("working", "blocked"):
            self.rozi.finish_turn(1, state)
            self.loop.reconcile()
        self.assertEqual(self.rozi.test_runs, [])
        self.rozi.test_replies = [passed()]
        self.rozi.finish_turn(1, "idle")
        self.loop.reconcile()
        self.assertEqual(len(self.rozi.test_runs), 1)

    def test_a_busy_action_is_tried_again(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [Reply(False, code="action-busy"), passed()]
        self.loop.reconcile()
        self.assertEqual(self.rozi.notes, [])
        self.loop.reconcile()
        self.assertEqual(self.rozi.notes, [("project: tests pass", "info")])

    def test_a_failed_call_is_reported_and_not_prompted(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [Reply(False, code="invocation-timeout", error="deadline")]
        self.loop.reconcile()
        self.assertEqual(self.rozi.prompts, [])
        self.assertEqual(self.rozi.notes, [("project: could not run the tests: deadline", "error")])

    def test_a_refused_prompt_is_reported(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [failed()]
        self.rozi.prompt_reply = Reply(False, code="agent-blocked", error="agent is blocked")
        self.loop.reconcile()
        (note,) = self.rozi.notes
        self.assertEqual(note[1], "error")
        self.assertIn("agent is blocked", note[0])
        self.assert_attempts(0)

    def test_refused_prompts_never_use_up_the_attempts(self) -> None:
        self.rozi.prompt_reply = Reply(False, code="agent-blocked", error="agent is blocked")
        self.rozi.test_replies = [failed(), failed(), failed()]
        for completions in (1, 2, 3):
            self.rozi.finish_turn(completions)
            self.loop.reconcile()
        self.assert_attempts(0)
        self.assertFalse(any("still fail" in note[0] for note in self.rozi.notes))

        self.rozi.prompt_reply = Reply(True)
        self.rozi.test_replies = [failed()]
        self.rozi.finish_turn(4)
        self.loop.reconcile()
        self.assertIn("attempt 1 of 2", self.rozi.prompts[-1])

    def test_a_refused_attempt_stays_unspent_after_a_restart(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [failed()]
        self.rozi.prompt_reply = Reply(False, code="agent-blocked", error="agent is blocked")
        self.loop.reconcile()
        self.loop = self.make_loop()
        self.assert_attempts(0)

    def test_a_prompt_with_no_answer_may_have_arrived_so_it_stays_counted(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [failed()]
        self.rozi.prompt_reply = Reply(False, error="no answer from rozi")
        self.loop.reconcile()
        self.assert_attempts(1)
        self.assertIn("got no answer", self.rozi.notes[0][0])

    def test_an_agent_outside_the_roots_is_left_alone(self) -> None:
        self.rozi.cwd = "/somewhere/else"
        self.rozi.finish_turn(1)
        self.loop.reconcile()
        self.assertEqual(self.rozi.test_runs, [])
        self.assertEqual(self.rozi.notes, [])

    def waiting_notes(self) -> list[str]:
        return [note for note, _ in self.rozi.notes if "waiting for approval" in note]

    def test_an_agent_it_asked_says_once_per_request_that_it_is_waiting(self) -> None:
        self.rozi.test_replies = [failed(), failed()]
        self.rozi.finish_turn(1)
        self.loop.reconcile()
        self.rozi.finish_turn(1, "blocked")
        self.loop.reconcile()
        self.loop.reconcile()
        self.assertEqual(len(self.waiting_notes()), 1)

        self.rozi.finish_turn(1, "working")
        self.loop.reconcile()
        self.rozi.finish_turn(1, "blocked")
        self.loop.reconcile()
        self.assertEqual(len(self.waiting_notes()), 1, "another approval in the same request")

        self.rozi.finish_turn(2)
        self.loop.reconcile()
        self.rozi.finish_turn(2, "blocked")
        self.loop.reconcile()
        self.assertEqual(len(self.waiting_notes()), 2, "a new request")

    def test_a_restart_does_not_repeat_the_waiting_note(self) -> None:
        self.rozi.test_replies = [failed()]
        self.rozi.finish_turn(1)
        self.loop.reconcile()
        self.rozi.finish_turn(1, "blocked")
        self.loop.reconcile()
        self.make_loop().reconcile()
        self.assertEqual(len(self.waiting_notes()), 1)

    def test_a_note_is_due_again_after_the_tests_pass(self) -> None:
        self.rozi.test_replies = [failed(), passed(), failed()]
        for completions in (1, 2, 3):
            self.rozi.finish_turn(completions)
            self.loop.reconcile()
            self.rozi.finish_turn(completions, "blocked")
            self.loop.reconcile()
        self.assertEqual(len(self.waiting_notes()), 2)

    def test_an_agent_it_never_asked_is_not_its_business_when_blocked(self) -> None:
        self.rozi.finish_turn(0, "blocked")
        self.loop.reconcile()
        self.assertEqual(self.rozi.notes, [])

    def test_a_gone_agent_is_forgotten(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [passed()]
        self.loop.reconcile()
        self.rozi.listed = []
        self.loop.reconcile()
        self.assertEqual(self.loop.ledger.agents, {})

    def test_an_unreadable_agent_list_changes_nothing(self) -> None:
        self.rozi.finish_turn(1)
        self.rozi.test_replies = [passed()]
        self.loop.reconcile()
        before = dict(self.loop.ledger.agents)
        self.rozi.listed = None
        self.loop.reconcile()
        self.assertEqual(self.loop.ledger.agents, before)


class RunTestsActionTest(unittest.TestCase):
    def setUp(self) -> None:
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.base = Path(directory.name)
        self.root = self.base / "with space ü"
        self.root.mkdir()

    def run_command(self, command: list[str], cwd: Path | None = None) -> dict:
        settings = Settings([str(self.root)], command, 3)
        return test_loop.run_tests(str(cwd or self.root), settings, self.base / "state")

    def test_success_and_failure_report_the_exit_code(self) -> None:
        result = self.run_command([sys.executable, "-c", "print('fine')"])
        self.assertEqual((result["ran"], result["passed"], result["exit_code"]), (True, True, 0))
        self.assertIn("fine", result["tail"])
        result = self.run_command([sys.executable, "-c", "raise SystemExit(3)"])
        self.assertEqual((result["passed"], result["exit_code"]), (False, 3))

    def test_output_goes_to_the_log_with_both_streams(self) -> None:
        script = "import sys; print('out'); print('err', file=sys.stderr)"
        result = self.run_command([sys.executable, "-c", script])
        text = Path(result["log"]).read_text(encoding="utf-8")
        self.assertIn("out", text)
        self.assertIn("err", text)

    def test_the_tail_is_bounded(self) -> None:
        result = self.run_command([sys.executable, "-c", "print('x' * 100000)"])
        self.assertLessEqual(len(result["tail"]), test_loop.TAIL_CHARS)

    def test_nothing_runs_outside_the_roots(self) -> None:
        marker = self.base / "ran"
        outside = self.base / "outside"
        outside.mkdir()
        result = self.run_command([sys.executable, "-c", f"open({str(marker)!r}, 'w')"], outside)
        self.assertEqual((result["ran"], result["passed"]), (False, False))
        self.assertFalse(marker.exists())

    def test_a_symlink_out_of_a_root_is_outside_it(self) -> None:
        outside = self.base / "outside"
        outside.mkdir()
        link = self.root / "link"
        os.symlink(outside, link)
        self.assertFalse(self.run_command([sys.executable, "-c", ""], link)["ran"])

    def test_a_sibling_sharing_the_prefix_is_outside(self) -> None:
        self.assertFalse(test_loop.within_roots("/src/app-old", ["/src/app"]))
        self.assertTrue(test_loop.within_roots("/src/app/sub", ["/src/app"]))

    def test_no_command_and_a_missing_program_are_answers(self) -> None:
        self.assertFalse(self.run_command([])["ran"])
        result = self.run_command(["/no/such/program"])
        self.assertFalse(result["ran"])
        self.assertIn("could not run", result["reason"])


class HelpersTest(unittest.TestCase):
    def test_only_the_snapshot_and_an_agent_stopping_wake_the_service(self) -> None:
        event = lambda state: json.dumps({"type": "event", "data": {"state": state}})  # noqa: E731
        self.assertTrue(test_loop.should_wake('{"type":"snapshot"}'))
        self.assertTrue(test_loop.should_wake(event("done")))
        self.assertTrue(test_loop.should_wake(event("idle")))
        self.assertTrue(test_loop.should_wake(event("blocked")))
        self.assertFalse(test_loop.should_wake(event("working")))
        self.assertFalse(test_loop.should_wake('{"type":"lagged"}'))
        self.assertFalse(test_loop.should_wake("not json"))

    def test_settings_default_to_doing_nothing(self) -> None:
        settings = test_loop.load_settings(None)
        self.assertEqual((settings.roots, settings.command), ([], []))

    def test_old_ledgers_are_removed_and_recent_ones_kept(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            old, recent, other = base / "agents-a.json", base / "agents-b.json", base / "logs.json"
            for path in (old, recent, other):
                path.write_text("{}", encoding="utf-8")
            os.utime(old, (0, 0))
            os.utime(other, (0, 0))
            test_loop.prune_ledgers(base, time.time())
            self.assertEqual((old.exists(), recent.exists(), other.exists()), (False, True, True))

    def test_a_damaged_ledger_starts_empty(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "agents-a.json"
            path.write_text("{not json", encoding="utf-8")
            self.assertEqual(AgentLedger(path).agents, {})


class ManifestTest(unittest.TestCase):
    def test_everything_runs_in_the_session_from_the_extensions_own_files(self) -> None:
        try:
            import tomllib
        except ModuleNotFoundError:
            self.skipTest("tomllib needs Python 3.11")
        root = Path(__file__).resolve().parent.parent
        manifest = tomllib.loads((root / "extension.toml").read_text(encoding="utf-8"))
        (service,) = manifest["services"]
        (action,) = manifest["actions"]
        self.assertEqual(service["placement"], "session")
        self.assertEqual(action["id"], "run-tests")
        self.assertEqual(manifest["settings"]["roots"], [])
        self.assertEqual(manifest["settings"]["command"], [])


if __name__ == "__main__":
    unittest.main()
