"""Storage, rows, and settings tests for the snippets extension.

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

import snippets  # noqa: E402


class IsolatedState(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary_directory = tempfile.TemporaryDirectory()
        self.saved_state = os.environ.get("XDG_STATE_HOME")
        self.saved_config = os.environ.get("ROZI_EXTENSION_CONFIG")
        os.environ["XDG_STATE_HOME"] = self.temporary_directory.name
        os.environ.pop("ROZI_EXTENSION_CONFIG", None)

    def tearDown(self) -> None:
        if self.saved_state is None:
            os.environ.pop("XDG_STATE_HOME", None)
        else:
            os.environ["XDG_STATE_HOME"] = self.saved_state
        if self.saved_config is None:
            os.environ.pop("ROZI_EXTENSION_CONFIG", None)
        else:
            os.environ["ROZI_EXTENSION_CONFIG"] = self.saved_config
        self.temporary_directory.cleanup()

    def test_add_round_trips_through_the_state_file(self) -> None:
        added = snippets.add_snippet("  git status  ")
        loaded = snippets.load_saved()
        self.assertEqual([item.text for item in loaded], ["git status"])
        self.assertEqual(loaded[0].id, added.id)
        self.assertTrue(loaded[0].saved)
        directory = snippets.commands_dir()
        self.assertIsNotNone(directory)
        assert directory is not None
        path = directory / f"{added.id}.json"
        self.assertTrue(path.is_file())
        self.assertEqual(directory.name, "commands")
        self.assertEqual(directory.parent.name, "rozi-snippets")

    def test_adds_and_deletes_are_independent_files(self) -> None:
        first = snippets.add_snippet("echo one")
        second = snippets.add_snippet("echo two")
        directory = snippets.commands_dir()
        assert directory is not None
        self.assertEqual(
            {path.name for path in directory.glob("*.json")},
            {f"{first.id}.json", f"{second.id}.json"},
        )
        snippets.delete_snippet(first.id, {first.id: first, second.id: second})
        self.assertEqual([item.text for item in snippets.load_saved()], ["echo two"])
        self.assertFalse((directory / f"{first.id}.json").exists())
        self.assertTrue((directory / f"{second.id}.json").is_file())

    def test_empty_or_duplicate_add_is_rejected(self) -> None:
        with self.assertRaisesRegex(snippets.ToolError, "empty"):
            snippets.valid_text("   ")
        snippets.add_snippet("cargo test")
        with self.assertRaisesRegex(snippets.ToolError, "already saved"):
            snippets.add_snippet("cargo test")

    def test_delete_only_removes_saved_rows(self) -> None:
        saved = snippets.add_snippet("echo one")
        config_row = snippets.Snippet("config:0", "git diff", False)
        items = {saved.id: saved, config_row.id: config_row}
        with self.assertRaisesRegex(snippets.ToolError, "Config snippets"):
            snippets.delete_snippet(config_row.id, items)
        snippets.delete_snippet(saved.id, items)
        self.assertEqual(snippets.load_saved(), [])

    def test_rows_group_config_and_saved_and_are_empty_without_a_fake_item(self) -> None:
        self.assertEqual(snippets.rows([]), [])
        request = snippets.picker_request([])
        self.assertEqual(request["empty"], "No snippets yet")
        self.assertEqual(request["rows"], [])
        self.assertEqual(request["actions"][0]["prompt"]["title"], "Command")
        mixed = [
            snippets.Snippet("config:0", "git status", False),
            snippets.Snippet("abc", "cargo test", True),
        ]
        by_id = {row["id"]: row for row in snippets.rows(mixed)}
        self.assertEqual(by_id["config:0"]["group"], "From config")
        self.assertEqual(by_id["abc"]["group"], "Saved")
        self.assertNotIn("disabled", by_id["abc"])

    def test_settings_fall_back_and_keep_string_lists(self) -> None:
        for value in (None, "", "not json", "[]"):
            if value is None:
                os.environ.pop("ROZI_EXTENSION_CONFIG", None)
            else:
                os.environ["ROZI_EXTENSION_CONFIG"] = value
            config = snippets.settings()
            self.assertIs(config["submit"], False)
            self.assertEqual(config["commands"], [])
        os.environ["ROZI_EXTENSION_CONFIG"] = json.dumps(
            {"submit": True, "commands": ["git status", 1, "git diff"]}
        )
        config = snippets.settings()
        self.assertIs(config["submit"], True)
        self.assertEqual(config["commands"], ["git status", "git diff"])
        self.assertEqual(
            [item.id for item in snippets.all_snippets(config)],
            ["config:0", "config:1"],
        )

    def test_picker_events_add_paste_and_cancel(self) -> None:
        items = {item.id: item for item in [snippets.add_snippet("echo hi")]}
        saved_id = next(iter(items))
        outcome, text = snippets.apply_picker_event({"cancelled": True}, items)
        self.assertEqual((outcome, text), ("done", None))
        outcome, text = snippets.apply_picker_event(
            {"action": "create", "input": "cargo test"}, items
        )
        self.assertEqual(outcome, "refresh")
        self.assertEqual(
            [item.text for item in snippets.load_saved()], ["echo hi", "cargo test"]
        )
        outcome, text = snippets.apply_picker_event({"selected": saved_id}, items)
        self.assertEqual((outcome, text), ("paste", "echo hi"))
        with self.assertRaisesRegex(snippets.ToolError, "Nothing to delete"):
            snippets.apply_picker_event({"action": "delete", "selected": "_empty"}, items)


if __name__ == "__main__":
    unittest.main()
