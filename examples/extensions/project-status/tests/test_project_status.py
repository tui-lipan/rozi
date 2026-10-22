"""Tests for the project-status extension, with state in a temporary directory.

Run from the extension directory:

    python -m unittest discover -s tests
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

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
