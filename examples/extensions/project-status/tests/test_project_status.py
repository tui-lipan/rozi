"""Tests for the project-status extension, with state in a temporary directory.

Run from the extension directory:

    python3 -m unittest discover -s tests
"""

from __future__ import annotations

import io
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bin"))

import project_status  # noqa: E402
from project_status import DirtySince, Status  # noqa: E402

PORCELAIN = "## main...origin/main [ahead 1]\n M src/lib.rs\n?? notes.md\n"


class ManifestTest(unittest.TestCase):
    def test_git_service_runs_on_the_active_session_host(self) -> None:
        try:
            import tomllib
        except ModuleNotFoundError:
            self.skipTest("tomllib needs Python 3.11")
        root = Path(__file__).resolve().parent.parent
        manifest = tomllib.loads((root / "extension.toml").read_text(encoding="utf-8"))
        (service,) = manifest["services"]
        self.assertEqual(service["placement"], "active-session")
        self.assertEqual(service["exec"][0], "python3")


class ParseTest(unittest.TestCase):
    def test_branch_and_changed_files(self) -> None:
        status = project_status.parse_status("/src/app", PORCELAIN)
        self.assertEqual(status, Status("/src/app", "main", ["src/lib.rs", "notes.md"]))

    def test_a_new_repository_names_its_branch(self) -> None:
        status = project_status.parse_status("/src/app", "## No commits yet on trunk\n")
        self.assertEqual((status.branch, status.changed), ("trunk", []))


class SnapshotTest(unittest.TestCase):
    def test_outside_a_repository_there_is_no_row(self) -> None:
        self.assertEqual(project_status.snapshot(None, None, 0), {"rows": []})

    def test_a_clean_project_is_idle(self) -> None:
        (row,) = project_status.snapshot(Status("/src/app", "main", []), None, 0)["rows"]
        self.assertEqual((row["title"], row["status"], row["reason"]), ("app · main", "idle", "clean"))

    def test_a_dirty_project_says_for_how_long(self) -> None:
        status = Status("/src/app", "feature", ["a"])
        (row,) = project_status.snapshot(status, 0, 3 * 3600 + 5)["rows"]
        self.assertEqual((row["status"], row["reason"]), ("working", "1 uncommitted file for 3h"))


class FocusTest(unittest.TestCase):
    def test_unknown_focus_never_falls_back_to_the_first_pane(self) -> None:
        with patch.object(project_status.subprocess, "run") as command:
            self.assertIsNone(project_status.focused_pane(None))
        command.assert_not_called()

    def test_missing_focus_or_invalid_layout_has_no_initial_pane(self) -> None:
        for payload in ('{}', '{"client": null}', '{"client": {"focused_pane": null}}', 'invalid'):
            with self.subTest(payload=payload), patch.object(
                project_status.subprocess, "run", return_value=SimpleNamespace(stdout=payload)
            ):
                self.assertIsNone(project_status.current_focus())

    def test_pane_lookup_rejects_a_different_session_with_the_same_id(self) -> None:
        payload = [{"session": "B", "id": 5, "cwd": "/repo/B"}]
        with patch.object(project_status.subprocess, "run", return_value=SimpleNamespace(
            stdout=json.dumps(payload)
        )):
            self.assertIsNone(project_status.focused_pane(("A", "5")))
            self.assertEqual(project_status.focused_pane(("B", "5")), payload[0])

    def test_polling_recovers_a_missed_focus_change_and_ignores_stale_events(self) -> None:
        # An empty stream models a transition before subscription registration. A stale
        # pane-1 event must not override layout's authoritative pane-5 snapshot either.
        cases = (
            (None, (("A", 5), ("A", 7))),
            ("1", (("A", 5), ("A", 7))),
            (None, (("A", 5), ("B", 5))),
        )
        for event_pane, targets in cases:
            with (
                self.subTest(event_pane=event_pane, targets=targets),
                tempfile.TemporaryDirectory() as state_dir,
            ):
                publishers = [Mock(), Mock()]
                for child in publishers:
                    child.poll.return_value = None
                event = {"event": "focus-changed", "data": {"pane": event_pane}}
                events = Mock(stdout=io.StringIO(json.dumps(event) + "\n" if event_pane else ""))
                events.poll.side_effect = [None, None, 0]
                focus = iter(targets)
                visible_session = ["A"]

                def query(arguments, **kwargs):
                    if arguments[1] == "layout":
                        session, pane = next(focus)
                        visible_session[0] = session
                        payload = {"session": session, "client": {"focused_pane": pane}}
                    else:
                        session = visible_session[0]
                        payload = [
                            {"id": pane, "session": session, "cwd": f"/repo/{session}/{pane}",
                             "reference": {"session_instance": f"instance-{session}"}}
                            for pane in (1, 5, 7)
                        ]
                    return SimpleNamespace(stdout=json.dumps(payload))

                # Drain any queued events before polling, without depending on thread timing.
                def reader(target, **kwargs):
                    return SimpleNamespace(start=target)

                def git_status(directory):
                    return Status(directory, "main", [])

                with (
                    patch.dict(project_status.os.environ, {
                        "ROZI_EXTENSION": "project-status",
                        "ROZI_EXTENSION_STATE_DIR": state_dir,
                    }),
                    patch.object(
                        project_status.subprocess, "Popen", side_effect=[events, *publishers]
                    ) as commands,
                    patch.object(project_status.subprocess, "run", side_effect=query),
                    patch.object(project_status.threading, "Thread", side_effect=reader),
                    patch.object(project_status.threading, "Event"),
                    patch.object(project_status, "git_status", side_effect=git_status) as git,
                ):
                    self.assertEqual(project_status.run(), 0)
                self.assertEqual(
                    [call.args[0] for call in git.call_args_list],
                    [f"/repo/{session}/{pane}" for session, pane in targets],
                )
                environments = [call.kwargs["env"] for call in commands.call_args_list[1:]]
                self.assertEqual(
                    [(env["ROZI_PANE"], env["ROZI_SESSION_INSTANCE"]) for env in environments],
                    [(str(pane), f"instance-{session}") for session, pane in targets],
                )
                titles = [
                    json.loads(child.stdin.write.call_args.args[0])["rows"][0]["title"]
                    for child in publishers
                ]
                self.assertEqual(titles, [f"{pane} · main" for _, pane in targets])
                for child in publishers:
                    child.stdin.close.assert_called_once()
                    child.wait.assert_called_once_with(timeout=2)
                events.terminate.assert_called_once()

    def test_publisher_failure_exits_for_supervised_retry_and_closes_children(self) -> None:
        publisher = Mock()
        publisher.publish.return_value = False
        events = Mock(stdout=io.StringIO())
        events.poll.return_value = None
        with (
            tempfile.TemporaryDirectory() as state_dir,
            patch.dict(project_status.os.environ, {
                "ROZI_EXTENSION": "project-status", "ROZI_EXTENSION_STATE_DIR": state_dir,
            }),
            patch.object(project_status, "Publisher", return_value=publisher),
            patch.object(project_status.subprocess, "Popen", return_value=events),
            patch.object(project_status, "current_focus", return_value=None),
        ):
            self.assertEqual(project_status.run(), 1)
        publisher.close.assert_called_once()
        events.terminate.assert_called_once()
        events.wait.assert_called_once_with(timeout=2)


class PublisherTest(unittest.TestCase):
    def test_ownership_follows_panes_sessions_and_session_instances(self) -> None:
        snapshots = []
        children = []

        def start(arguments, **kwargs):
            if children:
                # Old rows must be withdrawn before opening their replacement.
                children[-1].stdin.close.assert_called_once()
                children[-1].wait.assert_called_once_with(timeout=2)
            child = Mock()
            child.poll.return_value = None
            children.append(child)
            snapshots.append(kwargs["env"])
            return child

        publisher = project_status.Publisher()
        targets = (("A", 5, "instance-a"), ("A", 7, "instance-a"),
                   ("B", 7, "instance-b"), ("B", 7, "replacement-b"))
        with (
            patch.dict(project_status.os.environ, {
                "ROZI_PANE": "99", "ROZI_SESSION_INSTANCE": "launch-session",
                "ROZI_EXTENSION": "project-status",
            }),
            patch.object(project_status.subprocess, "Popen", side_effect=start) as command,
        ):
            for session, pane, instance in targets:
                target = {"session": session, "id": pane,
                          "reference": {"session_instance": instance}}
                rows = {"rows": [{"id": "project", "title": f"{session}/{pane}"}]}
                self.assertTrue(publisher.publish(target, rows))
                # Refreshing content in the same pane keeps the stream.
                self.assertTrue(publisher.publish(target, rows))
                self.assertEqual(json.loads(children[-1].stdin.write.call_args.args[0]), rows)
            self.assertTrue(publisher.publish(None, {"rows": []}))
            publisher.close()
        self.assertEqual(command.call_count, len(targets))
        self.assertEqual(
            [(env["ROZI_PANE"], env["ROZI_SESSION_INSTANCE"]) for env in snapshots],
            [(str(pane), instance) for _, pane, instance in targets],
        )
        self.assertTrue(all(env["ROZI_EXTENSION"] == "project-status" for env in snapshots))
        for child in children:
            child.stdin.close.assert_called_once()
            child.wait.assert_called_once_with(timeout=2)

    def test_missing_focus_does_not_open_an_untargeted_publisher(self) -> None:
        with patch.object(project_status.subprocess, "Popen") as command:
            self.assertTrue(project_status.Publisher().publish(None, {"rows": []}))
        command.assert_not_called()

    def test_a_slow_publisher_is_killed_and_reaped(self) -> None:
        child = Mock()
        child.poll.return_value = None
        child.wait.side_effect = [subprocess.TimeoutExpired("rozi publish", 2), 0]
        with patch.object(project_status.subprocess, "Popen", return_value=child):
            publisher = project_status.Publisher()
            publisher.publish({"session": "A", "id": 5}, {"rows": []})
            publisher.close()
        child.stdin.close.assert_called_once()
        child.kill.assert_called_once()
        self.assertEqual(child.wait.call_count, 2)


class DirtySinceTest(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.state_dir = Path(self.temporary_directory.name)

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    def test_the_first_dirty_time_survives_a_restart_and_clears_when_clean(self) -> None:
        dirty = Status("/src/app", "main", ["a"])
        self.assertEqual(DirtySince(self.state_dir).update(dirty, 100), 100)
        restarted = DirtySince(self.state_dir)
        self.assertEqual(restarted.update(dirty, 500), 100)
        self.assertIsNone(restarted.update(Status("/src/app", "main", []), 600))
        self.assertEqual(DirtySince(self.state_dir).update(dirty, 700), 700)
        self.assertEqual(list(self.state_dir.iterdir()), [self.state_dir / "dirty-since.json"])

    def test_a_damaged_state_file_starts_over(self) -> None:
        (self.state_dir / "dirty-since.json").write_text("{not json")
        self.assertEqual(DirtySince(self.state_dir).update(Status("/r", "main", ["a"]), 9), 9)


@unittest.skipUnless(shutil.which("git"), "git is not installed")
class GitTest(unittest.TestCase):
    def test_a_real_repository(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            def git(*arguments: str) -> None:
                subprocess.run(["git", "-C", directory, *arguments], check=True, capture_output=True)

            git("init", "-q", "-b", "work")
            (Path(directory) / "file.txt").write_text("x")
            status = project_status.git_status(directory)
            assert status is not None
            self.assertEqual(Path(status.root).resolve(), Path(directory).resolve())
            self.assertEqual((status.branch, status.changed), ("work", ["file.txt"]))
        with tempfile.TemporaryDirectory() as outside:
            self.assertIsNone(project_status.git_status(outside))


if __name__ == "__main__":
    unittest.main()
