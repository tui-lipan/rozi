"""Hook lifecycle tests with isolated state and a recorded public Rozi CLI."""

import concurrent.futures
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("activity", ROOT / "scripts/activity.py")
activity = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(activity)


class ActivityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rozi plugin ż ")
        self.addCleanup(self.temp.cleanup)
        self.env = {
            "ROZI": "1", "ROZI_PANE": "7", "ROZI_SOCKET": "ui-endpoint",
            "ROZI_SESSION_INSTANCE": "session-instance",
            "ROZI_BIN": str(Path(self.temp.name) / "rozi bin"),
            "CLAUDE_PLUGIN_DATA": self.temp.name,
        }
        # These tests exercise committed state, locking and delivery, not crash durability.
        # Avoid real disk flushes consuming the short production hook budgets on CI.
        connect = activity.connect_private

        def test_connection(path, timeout):
            db = connect(path, timeout)
            db.execute("PRAGMA synchronous = OFF")
            return db

        connections = patch.object(activity, "connect_private", side_effect=test_connection)
        connections.start()
        self.addCleanup(connections.stop)
        self.calls = []
        self.runner = patch.object(activity.subprocess, "run", side_effect=self.record)
        self.runner.start()
        self.addCleanup(self.runner.stop)

    def record(self, args, **kwargs):
        self.calls.append((args, kwargs))
        return subprocess.CompletedProcess(args, 0)

    def hook(self, name, session="native-id", pid=123, **extra):
        activity.handle({"hook_event_name": name, "session_id": session, **extra}, self.env, pid)

    def field(self, flag, call=-1):
        args = self.calls[call][0]
        return args[args.index(flag) + 1]

    def test_main_lifecycle_uses_one_token_and_increasing_sequences(self):
        for name in ("SessionStart", "UserPromptSubmit", "PermissionRequest", "PostToolBatch", "Stop", "SessionEnd"):
            self.hook(name)
        self.assertEqual([self.field("--state", i) for i in range(5)],
                         ["idle", "working", "blocked", "working", "done"])
        self.assertEqual([self.field("--seq", i) for i in range(6)], list(map(str, range(1, 7))))
        self.assertEqual(len({self.field("--integration", i) for i in range(6)}), 1)
        self.assertEqual(self.calls[-1][0][1:3], ["agents", "release"])
        self.assertNotIn("--native-session", self.calls[-1][0])

    def test_exit_tombstone_ignores_late_hooks(self):
        self.hook("SessionStart")
        self.hook("SessionEnd")
        self.hook("PostToolUse")
        self.hook("Stop")
        self.assertEqual(len(self.calls), 2)

    def test_resume_in_new_process_gets_fresh_token(self):
        self.hook("SessionStart")
        old = self.field("--integration")
        self.hook("SessionEnd")
        self.hook("SessionStart", pid=456, source="resume")
        self.assertNotEqual(old, self.field("--integration"))
        self.assertEqual(self.field("--seq"), "1")

    def test_clear_releases_and_starts_fresh_token(self):
        self.hook("SessionStart")
        old = self.field("--integration")
        self.hook("SessionEnd", reason="clear")
        self.hook("SessionStart", session="new-id", source="clear")
        self.assertNotEqual(old, self.field("--integration"))
        self.hook("Stop", session="native-id")
        self.assertEqual(len(self.calls), 3)

    def test_in_process_resume_releases_old_conversation_before_reporting_new_one(self):
        self.hook("SessionStart")
        token = self.field("--integration")
        self.hook("SessionStart", session="new-id", source="resume")
        self.assertEqual(self.calls[-2][0][1:3], ["agents", "release"])
        self.assertEqual(token, self.field("--integration", -2))
        self.assertNotEqual(token, self.field("--integration"))
        self.assertEqual(self.field("--native-session"), "new-id")
        self.hook("SessionEnd", session="native-id")
        self.assertEqual(len(self.calls), 3)

    def test_compaction_preserves_token_and_working_state(self):
        self.hook("SessionStart")
        token = self.field("--integration")
        self.hook("PreCompact")
        self.hook("SessionStart", source="compact")
        self.assertEqual(token, self.field("--integration"))
        self.assertEqual(self.field("--state"), "working")

    def test_subagent_hooks_cannot_change_parent(self):
        self.hook("SessionStart")
        for name in ("PreToolUse", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd"):
            self.hook(name, agent_id="child")
        self.hook("Stop", agent_transcript_path="child.jsonl")
        self.hook("SubagentStop")
        self.assertEqual(len(self.calls), 1)

    def test_stop_accounts_for_background_work_and_scheduled_wakeups(self):
        self.hook("SessionStart")
        self.hook("Stop", background_tasks=[{"id": "child"}])
        self.assertEqual(self.field("--state"), "working")
        self.hook("Stop", background_tasks=[], session_crons=[{"id": "cron"}])
        self.assertEqual(self.field("--state"), "working")
        self.hook("Stop", background_tasks=[], session_crons=[])
        self.assertEqual(self.field("--state"), "done")

    def test_questions_and_mcp_input_are_blocked_then_working(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_name="AskUserQuestion", tool_use_id="question")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_name="AskUserQuestion", tool_use_id="question")
        self.assertEqual(self.field("--state"), "working")
        self.hook("Elicitation")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("ElicitationResult")
        self.assertEqual(self.field("--state"), "working")

    def test_notifications_do_not_revive_parent_or_leak_content(self):
        self.hook("SessionStart")
        for kind in ("idle_prompt", "auth_success", "agent_completed", "agent_needs_input", "other"):
            self.hook("Notification", notification_type=kind)
        self.assertEqual(len(self.calls), 1)
        self.hook("Notification", notification_type="permission_prompt", message="secret command")
        self.assertEqual(self.field("--state"), "blocked")
        self.assertNotIn("secret command", str(self.calls))

    def test_missing_endpoint_or_plugin_data_does_not_touch_disk(self):
        for key in ("ROZI", "ROZI_PANE", "ROZI_SOCKET", "ROZI_SESSION_INSTANCE", "CLAUDE_PLUGIN_DATA"):
            env = self.env.copy()
            env.pop(key)
            activity.handle({"hook_event_name": "SessionStart", "session_id": "id"}, env, 123)
        self.assertEqual(self.calls, [])
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_missing_start_and_invalid_native_id_cannot_claim_pane(self):
        self.hook("UserPromptSubmit")
        self.hook("SessionStart", session=None)
        self.assertEqual(self.calls, [])

    def test_cli_uses_literal_argv_and_matching_environment(self):
        self.hook("SessionStart", session="literal ; $() ż")
        args, kwargs = self.calls[-1]
        self.assertEqual(args[0], self.env["ROZI_BIN"])
        self.assertEqual(self.field("--native-session"), "literal ; $() ż")
        self.assertNotIn("--session", args)
        self.assertNotIn("--target", args)
        self.assertEqual(kwargs["env"], self.env)
        self.assertGreater(kwargs["timeout"], 0)
        self.assertLessEqual(kwargs["timeout"], activity.CLI_TIMEOUT)
        self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
        self.assertEqual(kwargs["stderr"], subprocess.PIPE)

    def test_timeout_still_persists_sequence(self):
        self.hook("SessionStart")
        with patch.object(activity.subprocess, "run", side_effect=subprocess.TimeoutExpired("rozi", 0.75)):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.hook("UserPromptSubmit")
        self.hook("Stop")
        self.assertEqual(self.field("--seq"), "3")

    def test_concurrent_hooks_deliver_in_sequence_order(self):
        self.hook("SessionStart")
        original = self.record

        def slow_record(args, **kwargs):
            time.sleep(0.01)
            return original(args, **kwargs)

        with patch.object(activity.subprocess, "run", side_effect=slow_record):
            with concurrent.futures.ThreadPoolExecutor(max_workers=4) as pool:
                list(pool.map(lambda _: self.hook("PostToolUse"), range(4)))
        # Concurrent snapshots can be coalesced, but every delivered sequence advances and the
        # final report includes all four committed events.
        seqs = [int(self.field("--seq", i)) for i in range(len(self.calls))]
        self.assertEqual(seqs, sorted(set(seqs)))
        self.assertEqual(seqs[-1], 5)

    def test_parallel_permission_and_unrelated_completion_stay_blocked(self):
        for identified in (False, True):
            self.hook("SessionStart")
            extra = {"tool_use_id": "waiting"} if identified else {}
            barrier = threading.Barrier(2)

            def permission():
                barrier.wait()
                self.hook("PermissionRequest", **extra)

            def completion():
                barrier.wait()
                self.hook("PostToolUse", tool_use_id="other")

            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                futures = [pool.submit(permission), pool.submit(completion)]
                for future in futures:
                    future.result(timeout=2)
            self.assertEqual(self.field("--state"), "blocked")
            self.hook("PreToolUse", tool_use_id="third", tool_name="Read")
            self.assertEqual(self.field("--state"), "blocked")

    def test_twenty_parallel_hooks_preserve_waits_and_deliver_latest_state(self):
        self.hook("SessionStart")
        events = [
            ("PermissionRequest", {}),
            ("Elicitation", {"mcp_server_name": "server", "elicitation_id": "wait"}),
        ] + [("PostToolUse", {"tool_use_id": f"other-{index}"}) for index in range(18)]
        barrier = threading.Barrier(len(events))
        original = self.record

        def slow_record(args, **kwargs):
            time.sleep(0.01)
            return original(args, **kwargs)

        def run(event):
            barrier.wait(timeout=10)
            self.hook(event[0], **event[1])

        # This stress test checks concurrent state preservation, not hook latency. Allow slow
        # CI disks to serialize twenty SQLite writers and deliver their reports; separate tests
        # cover production persistence and delivery deadlines.
        with patch.object(activity, "STATE_RETRY_BUDGET", 30.0), \
             patch.object(activity, "DELIVERY_BUDGET", 30.0), \
             patch.object(activity.subprocess, "run", side_effect=slow_record):
            with concurrent.futures.ThreadPoolExecutor(max_workers=len(events)) as pool:
                list(pool.map(run, events))
        self.assertEqual(self.field("--state"), "blocked")
        self.assertEqual(self.field("--seq"), "21")
        db = activity.open_state(activity.state_path(self.env, 123))
        try:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM blockers").fetchone()[0], 2)
            self.assertEqual(db.execute("SELECT dirty FROM status").fetchone()[0], 0)
        finally:
            db.close()

    def test_completion_clears_only_its_own_identified_wait(self):
        self.hook("SessionStart")
        self.hook("PermissionRequest", tool_use_id="a")
        self.hook("PreToolUse", tool_name="AskUserQuestion", tool_use_id="b")
        self.hook("PostToolUse", tool_use_id="a")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUseFailure", tool_use_id="b")
        self.assertEqual(self.field("--state"), "working")

    def test_permission_without_id_waits_for_batch_even_if_same_tool_finishes(self):
        self.hook("SessionStart")
        self.hook("PermissionRequest", tool_name="Bash")
        self.hook("PostToolUse", tool_name="Bash", tool_use_id="unrelated")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolBatch", tool_calls=[])
        self.assertEqual(self.field("--state"), "working")

    def test_elicitation_result_clears_only_matching_server_and_request(self):
        self.hook("SessionStart")
        self.hook("Elicitation", mcp_server_name="one", elicitation_id="a")
        self.hook("Elicitation", mcp_server_name="two", elicitation_id="b")
        self.hook("PostToolUse", tool_use_id="unrelated")
        self.hook("ElicitationResult", mcp_server_name="one", elicitation_id="a")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("ElicitationResult", mcp_server_name="two", elicitation_id="b")
        self.assertEqual(self.field("--state"), "working")

    def test_two_elicitations_without_ids_need_two_responses(self):
        self.hook("SessionStart")
        self.hook("Elicitation", mcp_server_name="same")
        self.hook("Elicitation", mcp_server_name="same")
        self.hook("ElicitationResult", mcp_server_name="same")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("ElicitationResult", mcp_server_name="same")
        self.assertEqual(self.field("--state"), "working")

    def test_slow_delivery_does_not_lock_out_a_permission_event(self):
        self.hook("SessionStart")
        sending = threading.Event()
        unblock = threading.Event()
        original = self.record

        def held_delivery(args, **kwargs):
            if "--seq" in args and args[args.index("--seq") + 1] == "2":
                sending.set()
                self.assertTrue(unblock.wait(timeout=2))
            return original(args, **kwargs)

        with patch.object(activity.subprocess, "run", side_effect=held_delivery):
            with concurrent.futures.ThreadPoolExecutor(max_workers=2) as pool:
                sender = pool.submit(self.hook, "PostToolUse", tool_use_id="other")
                self.assertTrue(sending.wait(timeout=1))
                try:
                    permission = pool.submit(self.hook, "PermissionRequest")
                    permission.result(timeout=1)
                    db = activity.open_state(activity.state_path(self.env, 123))
                    try:
                        self.assertEqual(db.execute("SELECT state FROM status").fetchone()[0], "blocked")
                    finally:
                        db.close()
                finally:
                    unblock.set()
                sender.result(timeout=2)
        self.assertEqual(self.field("--state"), "blocked")

    def test_state_lock_longer_than_one_second_retries_without_losing_permission(self):
        self.hook("SessionStart")
        path = activity.state_path(self.env, 123)
        owner = activity.open_state(path)
        owner.execute("BEGIN IMMEDIATE")
        retrying = threading.Event()
        original = activity.retry_delay

        def observe_retry(deadline):
            retrying.set()
            return original(deadline)

        try:
            with patch.object(activity, "retry_delay", side_effect=observe_retry):
                with concurrent.futures.ThreadPoolExecutor(max_workers=1) as pool:
                    permission = pool.submit(self.hook, "PermissionRequest")
                    try:
                        self.assertTrue(retrying.wait(timeout=1))
                        # Exercise a real SQLite lock beyond the old one-second timeout.
                        time.sleep(1.1)
                    finally:
                        owner.commit()
                    permission.result(timeout=3)
        finally:
            owner.close()
        self.assertEqual(self.field("--state"), "blocked")
        self.assertEqual(self.field("--seq"), "2")

    def test_busy_transaction_rolls_back_before_retrying_the_event(self):
        self.hook("SessionStart")
        original = activity.record_event
        attempts = 0

        def fail_first_transaction(db, event, change):
            nonlocal attempts
            attempts += 1
            original(db, event, change)
            if attempts == 1:
                raise activity.sqlite3.OperationalError("database is locked")

        with patch.object(activity, "record_event", side_effect=fail_first_transaction):
            self.hook("Elicitation", mcp_server_name="server")
        db = activity.open_state(activity.state_path(self.env, 123))
        try:
            self.assertEqual(db.execute("SELECT count FROM blockers").fetchone()[0], 1)
        finally:
            db.close()
        self.assertEqual(attempts, 2)
        self.assertEqual(self.field("--seq"), "2")

    def test_state_retry_deadline_is_bounded_and_skips_delivery(self):
        error = activity.sqlite3.OperationalError("database is locked")
        with patch.object(activity, "persist_once", side_effect=error) as persist, \
             patch.object(activity.time, "monotonic", side_effect=[10, 14]):
            with self.assertRaises(activity.sqlite3.OperationalError):
                self.hook("SessionStart")
        self.assertEqual(persist.call_count, 1)
        self.assertEqual(self.calls, [])

    def test_non_lock_database_failure_is_not_retried(self):
        with patch.object(activity, "persist_once", side_effect=activity.sqlite3.OperationalError("disk I/O error")) as persist:
            with self.assertRaises(activity.sqlite3.OperationalError):
                self.hook("SessionStart")
        self.assertEqual(persist.call_count, 1)

    def test_resume_retries_release_after_timeout_launch_failure_or_nonzero_exit(self):
        failures = (
            subprocess.TimeoutExpired("rozi", 0.75), FileNotFoundError("rozi"),
            subprocess.CompletedProcess([], 1, stderr="session unavailable\n"),
        )
        for index, failure in enumerate(failures):
            pid = 600 + index
            self.hook("SessionStart", pid=pid)
            old_token = self.field("--integration")
            with patch.object(activity.subprocess, "run", side_effect=[failure]):
                if isinstance(failure, Exception):
                    with self.assertRaises(type(failure)):
                        self.hook("SessionStart", session="new-id", source="resume", pid=pid)
                else:
                    self.hook("SessionStart", session="new-id", source="resume", pid=pid)
            # New-session events still belong to the new lifecycle and retry the old release.
            self.hook("PermissionRequest", session="new-id", pid=pid)
            self.assertEqual(self.calls[-2][0][1:3], ["agents", "release"])
            self.assertEqual(self.field("--integration", -2), old_token)
            self.assertNotEqual(self.field("--integration"), old_token)
            self.assertEqual(self.field("--native-session"), "new-id")
            self.assertEqual(self.field("--state"), "blocked")

    def test_timed_out_release_that_reached_server_allows_new_claim(self):
        self.hook("SessionStart")
        with patch.object(activity.subprocess, "run", side_effect=subprocess.TimeoutExpired("rozi", 0.75)):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.hook("SessionStart", session="new-id", source="resume")
        refused = subprocess.CompletedProcess([], 1, stderr="integration token belongs to a retired agent incarnation\n")
        with patch.object(activity.subprocess, "run", side_effect=[refused, self.record_result()]):
            self.hook("UserPromptSubmit", session="new-id")
        db = activity.open_state(activity.state_path(self.env, 123))
        try:
            self.assertEqual(db.execute("SELECT session FROM run").fetchone()[0], "new-id")
            self.assertEqual(db.execute("SELECT COUNT(*) FROM releases").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT dirty FROM status").fetchone()[0], 0)
        finally:
            db.close()

    def record_result(self):
        return subprocess.CompletedProcess([], 0)

    def test_failed_exit_release_is_retried_when_next_conversation_starts(self):
        self.hook("SessionStart")
        old = self.field("--integration")
        with patch.object(activity.subprocess, "run", return_value=subprocess.CompletedProcess([], 1)):
            self.hook("SessionEnd", reason="resume")
        self.hook("SessionStart", session="new-id", source="resume")
        self.assertEqual(self.field("--integration", -2), old)
        self.assertEqual(self.field("--native-session"), "new-id")

    def test_retry_of_a_timed_out_report_uses_a_newer_sequence(self):
        self.hook("SessionStart")
        with patch.object(activity.subprocess, "run", side_effect=subprocess.TimeoutExpired("rozi", 0.75)):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.hook("UserPromptSubmit")
        path = activity.state_path(self.env, 123)
        db = activity.open_state(path)
        try:
            activity.flush_pending(db, path, self.env)
        finally:
            db.close()
        self.assertEqual(self.field("--seq"), "3")
        self.assertEqual(self.field("--state"), "working")

    def test_permission_notification_survives_unrelated_tool_completion(self):
        self.hook("SessionStart")
        self.hook("Notification", notification_type="permission_prompt")
        self.hook("PostToolUse", tool_use_id="other")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolBatch", tool_calls=[])
        self.assertEqual(self.field("--state"), "working")

    def test_parallel_batch_does_not_clear_a_usage_limit_wait(self):
        self.hook("SessionStart")
        self.hook("Notification", notification_type="quota_auto_resume_stale")
        self.hook("PostToolBatch", tool_calls=[])
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("Notification", notification_type="quota_auto_resume_fired")
        self.assertEqual(self.field("--state"), "working")

    def test_state_is_separate_for_each_server_and_pane(self):
        self.hook("SessionStart")
        first = self.field("--integration")
        self.env["ROZI_SESSION_INSTANCE"] = "another-server"
        self.hook("SessionStart")
        self.assertNotEqual(first, self.field("--integration"))
        self.env["ROZI_PANE"] = "8"
        self.hook("SessionStart")
        self.assertEqual(len(list(Path(self.temp.name).rglob("*.sqlite3"))), 3)

    def test_main_is_silent_and_successful_on_bad_input_and_cli_failure(self):
        for raw in ("bad json", "[]", '{"hook_event_name": [], "session_id": "id"}'):
            with patch.object(activity.sys, "stdin", io.StringIO(raw)), \
                 patch.object(activity.sys, "stdout", io.StringIO()) as out, \
                 patch.object(activity.sys, "stderr", io.StringIO()) as err, \
                 patch.dict(activity.os.environ, self.env, clear=True):
                activity.main()
                self.assertEqual(out.getvalue() + err.getvalue(), "")
        event = {"hook_event_name": "SessionStart", "session_id": "id"}
        with patch.object(activity.sys, "stdin", io.StringIO(json.dumps(event))), \
             patch.dict(activity.os.environ, self.env, clear=True), \
             patch.object(activity.subprocess, "run", side_effect=FileNotFoundError):
            activity.main()

    def test_manifest_hooks_run_directly_and_synchronously(self):
        hooks = json.loads((ROOT / "hooks/hooks.json").read_text())["hooks"]
        self.assertLess(activity.CLI_TIMEOUT, activity.DELIVERY_BUDGET)
        self.assertLess(activity.DELIVERY_BUDGET, activity.STATE_RETRY_BUDGET)
        for name, groups in hooks.items():
            hook = groups[0]["hooks"][0]
            self.assertEqual(hook["command"], "python")
            self.assertEqual(hook["args"], ["${CLAUDE_PLUGIN_ROOT}/scripts/activity.py"])
            self.assertFalse(hook.get("async", False), name)
            implementation_budget = (activity.STATE_RETRY_BUDGET + activity.DELIVERY_BUDGET
                                     + activity.DELIVERY_LOCK_TIMEOUT + 2 * activity.STATE_LOCK_TIMEOUT)
            self.assertGreater(hook["timeout"], implementation_budget + 0.5)
        self.assertNotIn("SubagentStop", hooks)
        self.assertIn("PostToolBatch", hooks)


if __name__ == "__main__":
    unittest.main()
