import unittest

from scripts.release_notes import (
    ReleaseNotesError,
    commit_summary,
    exclusion_reason,
    pull_request_url,
    select_previous_release,
    validate_notes,
)


class PreviousReleaseTests(unittest.TestCase):
    def test_selects_latest_published_v_tag_other_than_target(self):
        releases = [
            {
                "id": 4,
                "tag_name": "v0.0.23",
                "draft": False,
                "prerelease": True,
                "published_at": "2026-09-20T12:00:00Z",
            },
            {
                "id": 3,
                "tag_name": "nightly",
                "draft": False,
                "prerelease": True,
                "published_at": "2026-09-20T11:00:00Z",
            },
            {
                "id": 2,
                "tag_name": "v0.0.22",
                "draft": False,
                "prerelease": False,
                "published_at": "2026-09-19T12:00:00Z",
            },
            {
                "id": 1,
                "tag_name": "v0.0.21",
                "draft": True,
                "prerelease": False,
                "published_at": "2026-09-18T12:00:00Z",
            },
        ]

        self.assertEqual(select_previous_release(releases, "v0.0.23"), "v0.0.22")


class FilteringTests(unittest.TestCase):
    def test_filters_only_obvious_isolated_changes(self):
        self.assertEqual(
            exclusion_reason("ci: tune cache", [".github/workflows/ci.yml"]),
            "CI-only change",
        )
        self.assertEqual(
            exclusion_reason(
                "ci: write release notes from commit summaries",
                [".opencode/commands/changelog.md", "scripts/release_notes.py"],
            ),
            "CI-only change",
        )
        self.assertEqual(
            exclusion_reason("test: add fixture", ["tests/fixtures/input.txt"]),
            "test-only change",
        )
        self.assertEqual(
            exclusion_reason("docs: explain sessions", ["docs/sessions.md"]),
            "documentation-only change",
        )
        self.assertEqual(
            exclusion_reason(
                "chore: release 0.0.23",
                ["Cargo.toml", "Cargo.lock", "docs/installation.md"],
            ),
            "release metadata only",
        )

    def test_keeps_changes_that_may_be_user_visible(self):
        self.assertIsNone(exclusion_reason("perf: reduce redraws", ["src/view/pane.rs"]))
        self.assertIsNone(
            exclusion_reason(
                "chore(deps): update renderer",
                ["Cargo.toml", "Cargo.lock"],
            )
        )
        self.assertIsNone(
            exclusion_reason(
                "test: cover reconnect",
                ["src/session/remote.rs"],
            )
        )


class SummaryTests(unittest.TestCase):
    def test_keeps_the_summary_section_and_drops_checklists(self):
        body = """\
## Summary

Remote sessions now reconnect after a dropped SSH link.

## Checklist

- [x] `cargo test` passes

Signed-off-by: Ada <ada@example.com>
"""
        self.assertEqual(
            commit_summary(body),
            "Remote sessions now reconnect after a dropped SSH link.",
        )

    def test_uses_the_preamble_when_there_is_no_summary_heading(self):
        body = """\
Live System theme updates refresh all 16 ANSI slots.

## Notes for reviewers

This uses one persistent parser.

---------
Co-authored-by: Ada <ada@example.com>
"""
        self.assertEqual(
            commit_summary(body),
            "Live System theme updates refresh all 16 ANSI slots.",
        )

    def test_strips_html_comments(self):
        body = """\
<!-- PR titles land in master's history -->

## Summary

Extension pickers open without a delay.
"""
        self.assertEqual(
            commit_summary(body),
            "Extension pickers open without a delay.",
        )


class PullRequestUrlTests(unittest.TestCase):
    def test_reads_the_squash_merge_trailer(self):
        self.assertEqual(
            pull_request_url(
                "tui-lipan/rozi",
                "fix: show reconnect overlay when a remote session goes quiet (#20)",
            ),
            "https://github.com/tui-lipan/rozi/pull/20",
        )

    def test_ignores_subjects_without_a_trailer(self):
        self.assertIsNone(pull_request_url("tui-lipan/rozi", "chore: release 0.0.23"))


class ValidationTests(unittest.TestCase):
    def test_accepts_nonempty_sections_in_fixed_order(self):
        notes = """\
## Added

- Rozi now opens remote sessions.

## Fixed

- Remote sessions now reconnect after a dropped SSH link.
  Existing sessions remain offline when their server is gone.

## Compatibility

- Session protocol is now version 8; restart existing session servers after upgrading.
"""
        self.assertEqual(validate_notes(notes), notes)

    def test_accepts_pull_request_links(self):
        notes = """\
## Fixed

- Remote sessions now reconnect after a dropped SSH link.
  ([#20](https://github.com/tui-lipan/rozi/pull/20))
"""
        self.assertEqual(validate_notes(notes), notes)

    def test_rejects_unknown_or_reordered_sections(self):
        with self.assertRaises(ReleaseNotesError):
            validate_notes("## Fixed\n\n- One.\n\n## Added\n\n- Two.\n")
        with self.assertRaises(ReleaseNotesError):
            validate_notes("## Internal\n\n- Refactored modules.\n")

    def test_rejects_preamble_code_fences_and_empty_sections(self):
        with self.assertRaises(ReleaseNotesError):
            validate_notes("Release notes\n\n## Fixed\n\n- One.\n")
        with self.assertRaises(ReleaseNotesError):
            validate_notes("## Fixed\n\n```text\none\n```\n")
        with self.assertRaises(ReleaseNotesError):
            validate_notes("## Fixed\n")

    def test_rejects_ai_tell_punctuation(self):
        with self.assertRaises(ReleaseNotesError):
            validate_notes("## Fixed\n\n- Rozi reconnects — without replacing the session.\n")
        with self.assertRaises(ReleaseNotesError):
            validate_notes('## Changed\n\n- Rozi calls this mode “offline”.\n')


if __name__ == "__main__":
    unittest.main()
