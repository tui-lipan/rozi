from __future__ import annotations

import importlib.util
import json
import sys
import unittest
from pathlib import Path


SCRIPT = Path(__file__).parents[1] / "bin" / "pr_dashboard.py"


def load_script():
    spec = importlib.util.spec_from_file_location("pr_dashboard_example", SCRIPT)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"could not load {SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


pr_dashboard = load_script()


def gh_pr(payload: str) -> dict[str, object]:
    value = json.loads(payload)
    if not isinstance(value, dict):
        raise TypeError("fixture must be a JSON object")
    return value


class PullRequestSummaryTests(unittest.TestCase):
    def summarize(self, payload: str):
        result = pr_dashboard.summarize_pr(
            gh_pr(payload), "Current branch", current=True
        )
        self.assertIsNotNone(result)
        return result

    def test_failed_checks_are_blocked_and_report_passed_checks(self) -> None:
        pull_request = self.summarize(
            """
            {
              "number": 17,
              "title": "  Keep   status concise  ",
              "headRefName": "feature/checks",
              "url": "https://example.invalid/pull/17",
              "statusCheckRollup": [
                {"conclusion": "FAILURE"},
                {"state": "SUCCESS"},
                {"status": "COMPLETED", "conclusion": "SUCCESS"}
              ],
              "reviewDecision": "APPROVED",
              "mergeable": "MERGEABLE",
              "mergeStateStatus": "CLEAN"
            }
            """
        )

        self.assertEqual(pull_request.status, "blocked")
        self.assertEqual(pull_request.reason, "1 failed · 2 passed")
        self.assertEqual(pull_request.title, "Keep status concise")
        self.assertTrue(pull_request.current)

    def test_unknown_and_pending_check_states_are_counted_as_running(self) -> None:
        pull_request = self.summarize(
            """
            {
              "number": 18,
              "title": "Pending checks",
              "statusCheckRollup": [
                {"status": "IN_PROGRESS"},
                {"status": "A_NEW_GITHUB_STATE"},
                {"conclusion": "SUCCESS"}
              ]
            }
            """
        )

        self.assertEqual(pull_request.status, "working")
        self.assertEqual(pull_request.reason, "2 checks running · 1 passed")

    def test_merge_conflicts_take_precedence_over_review_and_checks(self) -> None:
        pull_request = self.summarize(
            """
            {
              "number": 19,
              "title": "Conflicted",
              "statusCheckRollup": [{"conclusion": "FAILURE"}],
              "reviewDecision": "CHANGES_REQUESTED",
              "mergeable": "CONFLICTING",
              "mergeStateStatus": "DIRTY"
            }
            """
        )

        self.assertEqual((pull_request.status, pull_request.reason), (
            "blocked",
            "merge conflicts",
        ))

    def test_approved_pull_request_without_checks_is_done(self) -> None:
        pull_request = self.summarize(
            """
            {
              "number": 20,
              "title": "No CI",
              "statusCheckRollup": null,
              "reviewDecision": "APPROVED"
            }
            """
        )

        self.assertEqual((pull_request.status, pull_request.reason), (
            "done",
            "approved · no checks",
        ))

    def test_incomplete_gh_row_is_ignored(self) -> None:
        self.assertIsNone(
            pr_dashboard.summarize_pr(
                {"number": "21", "title": "Wrong number type"},
                "Authored by you",
                False,
            )
        )


if __name__ == "__main__":
    unittest.main()
