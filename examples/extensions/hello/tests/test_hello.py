"""Tests for the hello extension, with `rozi` replaced by a recorder.

Run from the extension directory:

    python -m unittest discover -s tests
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
import unittest
from pathlib import Path
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "bin"))

import hello  # noqa: E402


class FakeRozi:
    """Answers `rozi pick` with a canned reply and records every call."""

    def __init__(self, pick_reply: str, pick_exit: int = 0) -> None:
        self.calls: list[tuple[list[str], str | None]] = []
        self.pick_reply = pick_reply
        self.pick_exit = pick_exit

    def __call__(self, argv, input=None, **_options):  # noqa: A002 - mirrors subprocess.run
        self.calls.append((list(argv[1:]), input))
        if argv[1] == "pick":
            return subprocess.CompletedProcess(argv, self.pick_exit, self.pick_reply, "")
        return subprocess.CompletedProcess(argv, 0, "", "")


class HelloTest(unittest.TestCase):
    def run_extension(self, fake: FakeRozi, *arguments: str) -> int:
        with (
            mock.patch.dict(os.environ, {"ROZI_EXTENSION": "hello"}),
            mock.patch.object(hello.subprocess, "run", fake),
        ):
            return hello.main(list(arguments))

    def test_greet_notifies_without_asking(self) -> None:
        fake = FakeRozi("")
        self.assertEqual(self.run_extension(fake), 0)
        self.assertEqual(fake.calls, [(["notify", "Hello, world!"], None)])

    def test_pick_offers_every_name_and_greets_the_chosen_one(self) -> None:
        fake = FakeRozi('{"selected":"Rozi"}\n')
        self.assertEqual(self.run_extension(fake, "--pick"), 0)
        (pick, document), notify = fake.calls
        self.assertEqual(pick, ["pick", "--json"])
        rows = json.loads(document)["rows"]
        self.assertEqual([row["id"] for row in rows], hello.NAMES)
        self.assertEqual(notify, (["notify", "Hello, Rozi!"], None))

    def test_a_cancelled_pick_says_nothing(self) -> None:
        fake = FakeRozi('{"cancelled":true}\n', pick_exit=1)
        self.assertEqual(self.run_extension(fake, "--pick"), 0)
        self.assertEqual([call[0][0] for call in fake.calls], ["pick"])

    def test_it_refuses_to_run_outside_rozi(self) -> None:
        fake = FakeRozi("")
        with (
            mock.patch.dict(os.environ, {}, clear=True),
            mock.patch.object(hello.subprocess, "run", fake),
        ):
            self.assertEqual(hello.main([]), 2)
        self.assertEqual(fake.calls, [])


if __name__ == "__main__":
    unittest.main()
