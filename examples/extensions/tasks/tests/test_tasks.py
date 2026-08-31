"""Discovery and settings tests for the tasks extension.

Run from the extension directory:

    python -m unittest discover -s tests
"""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bin"))

import tasks  # noqa: E402


class TempProject:
    def __init__(self, files: dict[str, str]):
        self.files = files

    def __enter__(self) -> Path:
        self.temp = tempfile.TemporaryDirectory()
        root = Path(self.temp.name)
        for name, body in self.files.items():
            path = root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")
        return root

    def __exit__(self, *_: object) -> None:
        self.temp.cleanup()


class DiscoveryTests(unittest.TestCase):
    def test_just_recipes_skip_comments_variables_and_bodies(self):
        justfile = (
            "# a comment\n"
            "export FOO := 'bar'\n"
            "build:\n"
            "    cargo build\n"
            "test target='all':\n"
            "    cargo test\n"
            "build:\n"  # duplicate name, kept once
        )
        with TempProject({"justfile": justfile}) as root:
            names = [task.name for task in tasks.just_tasks(root)]
        self.assertEqual(names, ["build", "test"])

    def test_make_targets_skip_variables_dot_targets_and_recipe_lines(self):
        makefile = (
            ".PHONY: build\n"
            "CFLAGS := -O2\n"
            "build: deps\n"
            "\tcc main.c\n"
            "deps:\n"
            "\techo deps\n"
        )
        with TempProject({"Makefile": makefile}) as root:
            names = [task.name for task in tasks.make_tasks(root)]
        self.assertEqual(names, ["build", "deps"])

    def test_package_scripts_pick_the_runner_from_the_lockfile(self):
        manifest = json.dumps({"scripts": {"dev": "vite", "build": "vite build"}})
        with TempProject({"package.json": manifest}) as root:
            self.assertEqual(tasks.npm_tasks(root)[0].command, ["npm", "run", "dev"])
        with TempProject({"package.json": manifest, "pnpm-lock.yaml": ""}) as root:
            self.assertEqual(tasks.npm_tasks(root)[0].command, ["pnpm", "run", "dev"])
        with TempProject({"package.json": manifest, "yarn.lock": ""}) as root:
            self.assertEqual(tasks.npm_tasks(root)[0].command, ["yarn", "dev"])

    def test_a_malformed_package_json_yields_no_tasks_instead_of_failing(self):
        with TempProject({"package.json": "{not json"}) as root:
            self.assertEqual(tasks.npm_tasks(root), [])

    def test_the_project_root_is_the_nearest_marker_above_the_pane(self):
        with TempProject({"justfile": "build:\n", "sub/deep/keep": ""}) as root:
            self.assertEqual(tasks.project_root(root / "sub" / "deep"), root)

    def test_sources_are_scanned_in_the_configured_order(self):
        files = {"justfile": "j:\n", "Makefile": "m:\n"}
        with TempProject(files) as root:
            forward = [task.source for task in tasks.discover(root, ["just", "make"])]
            reverse = [task.source for task in tasks.discover(root, ["make", "just"])]
            only = [task.source for task in tasks.discover(root, ["make"])]
        self.assertEqual(forward, ["just", "make"])
        self.assertEqual(reverse, ["make", "just"])
        self.assertEqual(only, ["make"])


class SettingsTests(unittest.TestCase):
    def setUp(self):
        self.saved = os.environ.get("ROZI_EXTENSION_CONFIG")

    def tearDown(self):
        if self.saved is None:
            os.environ.pop("ROZI_EXTENSION_CONFIG", None)
        else:
            os.environ["ROZI_EXTENSION_CONFIG"] = self.saved

    def test_settings_fall_back_when_the_variable_is_absent_or_unusable(self):
        for value in (None, "", "not json", "[]"):
            if value is None:
                os.environ.pop("ROZI_EXTENSION_CONFIG", None)
            else:
                os.environ["ROZI_EXTENSION_CONFIG"] = value
            config = tasks.settings()
            self.assertEqual(config["sources"], ["just", "make", "npm"])
            self.assertEqual(config["pane"], "focus")
            self.assertIs(config["keep_open"], True)

    def test_declared_values_are_read_with_their_json_types(self):
        os.environ["ROZI_EXTENSION_CONFIG"] = json.dumps(
            {"sources": ["make"], "pane": "background", "keep_open": False, "workspace": 9}
        )
        config = tasks.settings()
        self.assertEqual(config["sources"], ["make"])
        self.assertEqual(config["pane"], "background")
        self.assertIs(config["keep_open"], False)
        self.assertEqual(config["workspace"], 9)


class SidebarOutputTests(unittest.TestCase):
    def test_list_marks_each_section_with_the_declared_group_prefix(self):
        import io
        import contextlib

        with TempProject({"justfile": "build:\n", "Makefile": "deps:\n"}) as root:
            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                tasks.command_list(root, tasks.settings())
        # Rows carry the runnable command, not the bare name: a click sends the row as text.
        self.assertEqual(
            buffer.getvalue().splitlines(),
            ["## just", "just build", "## make", "make deps"],
        )

    def test_an_empty_project_prints_only_an_inert_header(self):
        import io
        import contextlib

        with TempProject({"keep": ""}) as root:
            buffer = io.StringIO()
            with contextlib.redirect_stdout(buffer):
                tasks.command_list(root, tasks.settings())
        lines = buffer.getvalue().splitlines()
        self.assertEqual(len(lines), 1)
        # Every line a command tab shows is clickable unless it starts with the group prefix.
        self.assertTrue(lines[0].startswith("## "), lines)


if __name__ == "__main__":
    unittest.main()
