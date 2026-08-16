from __future__ import annotations

import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
from contextlib import contextmanager
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "bin" / "git_tools.py"


def load_script():
    spec = importlib.util.spec_from_file_location("git_tools_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


git_tools = load_script()


@contextmanager
def working_directory(path: Path):
    previous = Path.cwd()
    os.chdir(path)
    try:
        yield
    finally:
        os.chdir(previous)


@unittest.skipUnless(shutil.which("git"), "Git is not installed")
class GitToolsTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.repo = Path(self.temporary_directory.name) / "repository"
        self.repo.mkdir()
        self.run_git("init")
        self.run_git("config", "user.name", "Rozi Tests")
        self.run_git("config", "user.email", "rozi-tests@example.invalid")
        self.run_git("symbolic-ref", "HEAD", "refs/heads/main")
        (self.repo / "tracked.txt").write_text("initial\n", encoding="utf-8")
        self.run_git("add", "tracked.txt")
        self.run_git("commit", "-m", "initial")
        self.run_git("branch", "feature")

    def tearDown(self) -> None:
        self.temporary_directory.cleanup()

    def run_git(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["git", "-C", str(self.repo), *args],
            check=True,
            capture_output=True,
            text=True,
        )

    def test_branch_rows_mark_current_protected_and_dirty_safely(self) -> None:
        with working_directory(self.repo):
            rows, branches = git_tools.branch_rows()

            by_name = {row["label"]: row for row in rows}
            self.assertEqual(by_name["main"]["group"], "Current")
            self.assertEqual(by_name["main"]["disabled"], "Current")
            self.assertTrue(by_name["main"]["active"])
            self.assertIn("protected", by_name["main"]["description"])
            self.assertEqual(by_name["feature"]["group"], "Recent")
            self.assertNotIn("disabled", by_name["feature"])
            self.assertEqual(set(branches), {"refs/heads/main", "refs/heads/feature"})

            (self.repo / "tracked.txt").write_text("dirty\n", encoding="utf-8")
            dirty_rows, _ = git_tools.branch_rows()

        dirty_by_name = {row["label"]: row for row in dirty_rows}
        self.assertEqual(dirty_by_name["feature"]["disabled"], "Dirty tree")
        self.assertEqual(dirty_by_name["main"]["disabled"], "Current · dirty")

    def test_branch_validation_and_delete_guards_do_not_mutate_repository(self) -> None:
        with working_directory(self.repo):
            with self.assertRaisesRegex(git_tools.ToolError, "already exists"):
                git_tools.valid_new_branch("feature")
            with self.assertRaisesRegex(git_tools.ToolError, "Invalid branch name"):
                git_tools.valid_new_branch("--upload-pack=malicious")

            current = git_tools.Branch(
                ref="refs/heads/main",
                name="main",
                current=True,
                age="now",
                worktree=str(self.repo),
                protected=True,
            )
            with self.assertRaisesRegex(git_tools.ToolError, "Current branch"):
                git_tools.delete_branch(current.ref, {current.ref: current})

            protected = git_tools.Branch(
                ref="refs/heads/main",
                name="main",
                current=False,
                age="now",
                worktree=None,
                protected=True,
            )
            with self.assertRaisesRegex(git_tools.ToolError, "Protected branch"):
                git_tools.delete_branch(protected.ref, {protected.ref: protected})

            self.assertEqual(
                set(self.run_git("branch", "--format=%(refname:short)").stdout.split()),
                {"feature", "main"},
            )


if __name__ == "__main__":
    unittest.main()
