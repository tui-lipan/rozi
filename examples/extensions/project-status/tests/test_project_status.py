"""Tests for the project-status extension, with state in a temporary directory.

Run from the extension directory:

    python -m unittest discover -s tests
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
            self.assertIsNone(project_status.focused_cwd(None))
        command.assert_not_called()

    def test_missing_focus_or_invalid_layout_has_no_initial_pane(self) -> None:
        for payload in ('{}', '{"client": null}', '{"client": {"focused_pane": null}}', 'invalid'):
            with self.subTest(payload=payload), patch.object(
                project_status.subprocess, "run", return_value=SimpleNamespace(stdout=payload)
            ):
                self.assertIsNone(project_status.current_focus())

    def test_first_publication_uses_initial_focus_and_queued_focus_changes(self) -> None:
        # Pane 1 comes first, but the UI starts on pane 5. A queued event can move it to 7.
        for event_pane, expected in ((None, "5"), ("7", "7")):
            with self.subTest(event_pane=event_pane), tempfile.TemporaryDirectory() as state_dir:
                output = io.StringIO()
                publisher = SimpleNamespace(stdin=output, poll=Mock(side_effect=[None, 0]))
                event = {"event": "focus-changed", "data": {"pane": event_pane}}
                events = SimpleNamespace(
                    stdout=io.StringIO(json.dumps(event) + "\n" if event_pane else ""),
                    poll=lambda: None,
                )
                commands = []

                def start(arguments, **kwargs):
                    commands.append(arguments[1:])
                    return publisher if arguments[1] == "publish" else events

                def query(arguments, **kwargs):
                    commands.append(arguments[1:])
                    if arguments[1] == "layout":
                        payload = {"client": {"focused_pane": 5}}
                    else:
                        payload = [{"id": pane, "cwd": f"/repo/{pane}"} for pane in (1, 5, 7)]
                    return SimpleNamespace(stdout=json.dumps(payload))

                # Drain the already queued events deterministically when the reader starts.
                def reader(target, **kwargs):
                    return SimpleNamespace(start=target)

                with (
                    patch.dict(project_status.os.environ, {
                        "ROZI_EXTENSION": "project-status",
                        "ROZI_EXTENSION_STATE_DIR": state_dir,
                    }),
                    patch.object(project_status.subprocess, "Popen", side_effect=start),
                    patch.object(project_status.subprocess, "run", side_effect=query),
                    patch.object(project_status.threading, "Thread", side_effect=reader),
                    patch.object(project_status.time, "sleep"),
                    patch.object(project_status, "git_status", return_value=Status(
                        f"/repo/{expected}", "main", []
                    )) as git,
                ):
                    self.assertEqual(project_status.run(), 0)
                git.assert_called_once_with(f"/repo/{expected}")
                (row,) = json.loads(output.getvalue())["rows"]
                self.assertEqual(row["title"], f"{expected} · main")
                self.assertLess(
                    commands.index(["subscribe", "focus-changed", "pane-exited"]),
                    commands.index(["layout", "get", "--format", "json"]),
                )


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
