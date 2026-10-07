"""Hook lifecycle tests with isolated state and a recorded public Rozi CLI."""

import concurrent.futures
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location("codex_activity", ROOT / "scripts/activity.py")
activity = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(activity)

BASH = {"tool_name": "Bash", "tool_input": {"command": "cargo test"}}


class ActivityTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rozi codex plugin ż ")
        self.addCleanup(self.temp.cleanup)
        self.env = {
            "ROZI": "1", "ROZI_PANE": "7", "ROZI_SOCKET": "ui-endpoint",
            "ROZI_SESSION_INSTANCE": "session-instance",
            "ROZI_BIN": str(Path(self.temp.name) / "rozi bin"),
            "PLUGIN_DATA": self.temp.name,
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

    def hook(self, name, session="thread-id", **extra):
        activity.handle({"hook_event_name": name, "session_id": session, **extra}, self.env)

    def field(self, flag, call=-1):
        args = self.calls[call][0]
        return args[args.index(flag) + 1]

    def permission(self, **call):
        call = {**BASH, **call}
        tool_input = {**call["tool_input"], "description": "Do you approve running cargo test?"}
        self.hook("PermissionRequest", tool_name=call["tool_name"], tool_input=tool_input)

    def test_main_lifecycle_uses_one_token_and_increasing_sequences(self):
        self.hook("SessionStart", source="startup")
        self.hook("UserPromptSubmit")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.permission()
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.hook("Stop")
        self.hook("SessionEnd", reason="other")
        self.assertEqual([self.field("--state", i) for i in range(6)],
                         ["idle", "working", "working", "blocked", "working", "done"])
        self.assertEqual([self.field("--seq", i) for i in range(7)], list(map(str, range(1, 8))))
        self.assertEqual(len({self.field("--integration", i) for i in range(7)}), 1)
        self.assertTrue(self.field("--integration").startswith("codex:"))
        self.assertEqual(self.field("--agent", 0), "codex")
        self.assertEqual(self.field("--native-session", 0), "thread-id")
        self.assertEqual(self.calls[-1][0][1:3], ["agents", "release"])
        self.assertNotIn("--native-session", self.calls[-1][0])

    def test_exit_tombstone_ignores_late_hooks(self):
        self.hook("SessionStart")
        self.hook("SessionEnd")
        self.hook("PostToolUse", **BASH)
        self.hook("Stop")
        self.hook("UserPromptSubmit")
        self.assertEqual(len(self.calls), 2)

    def test_exit_releases_only_the_thread_on_screen(self):
        # Codex ends every thread a TUI opened when it exits, the earlier ones included.
        self.hook("SessionStart")
        self.hook("SessionStart", session="second", source="resume")
        self.hook("SessionEnd", session="thread-id")
        self.assertEqual(self.field("--native-session"), "second")
        self.hook("SessionEnd", session="second")
        self.assertEqual(self.calls[-1][0][1:3], ["agents", "release"])
        self.assertEqual(self.field("--integration"), self.field("--integration", -2))

    def test_resume_in_new_process_gets_fresh_token(self):
        self.hook("SessionStart")
        old = self.field("--integration")
        self.hook("SessionEnd")
        self.hook("SessionStart", source="resume")
        self.assertNotEqual(old, self.field("--integration"))
        self.assertEqual(self.field("--seq"), "1")

    def test_in_process_resume_releases_old_thread_before_reporting_new_one(self):
        self.hook("SessionStart")
        token = self.field("--integration")
        self.hook("SessionStart", session="new-id", source="resume")
        self.assertEqual(self.calls[-2][0][1:3], ["agents", "release"])
        self.assertEqual(token, self.field("--integration", -2))
        self.assertNotEqual(token, self.field("--integration"))
        self.assertEqual(self.field("--native-session"), "new-id")
        self.hook("Stop", session="thread-id")
        self.assertEqual(len(self.calls), 3)

    def test_prompt_in_a_thread_started_earlier_claims_the_pane(self):
        # Returning to a thread this TUI already started fires no second SessionStart.
        self.hook("SessionStart")
        self.hook("SessionStart", session="second", source="resume")
        second = self.field("--integration")
        self.hook("UserPromptSubmit", session="thread-id")
        self.assertEqual(self.calls[-2][0][1:3], ["agents", "release"])
        self.assertEqual(self.field("--integration", -2), second)
        self.assertEqual(self.field("--native-session"), "thread-id")
        self.assertEqual(self.field("--state"), "working")

    def test_prompt_without_a_start_claims_the_pane(self):
        self.hook("UserPromptSubmit")
        self.assertEqual(self.field("--state"), "working")
        self.assertEqual(self.field("--seq"), "1")

    def test_tool_events_from_another_thread_cannot_claim_the_pane(self):
        self.hook("SessionStart")
        for name in ("PreToolUse", "PermissionRequest", "PostToolUse", "Stop", "Interrupt"):
            self.hook(name, session="background", **BASH)
        self.assertEqual(len(self.calls), 1)

    def test_compaction_preserves_token_and_working_state(self):
        self.hook("SessionStart")
        token = self.field("--integration")
        self.hook("PreCompact", trigger="auto")
        self.hook("SessionStart", source="compact")
        self.hook("PostCompact", trigger="auto")
        self.assertEqual({self.field("--integration", i) for i in range(4)}, {token})
        self.assertEqual(self.field("--state"), "working")

    def test_clear_starts_a_fresh_token_for_the_new_thread(self):
        self.hook("SessionStart")
        old = self.field("--integration")
        self.hook("SessionStart", session="cleared", source="clear")
        self.assertNotEqual(old, self.field("--integration"))
        self.assertEqual(self.field("--state"), "idle")

    def test_subagent_hooks_cannot_change_parent(self):
        self.hook("SessionStart")
        self.hook("UserPromptSubmit")
        self.hook("SubagentStart", agent_id="child", agent_type="default")
        self.hook("SubagentStop", agent_id="child", agent_transcript_path="child.jsonl")
        self.hook("Stop", agent_id="child")
        self.assertEqual(len(self.calls), 2)

    def test_interrupt_clears_waits_and_reports_idle(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.permission()
        self.hook("Interrupt")
        self.assertEqual(self.field("--state"), "idle")
        self.assertNotIn("--reason", self.calls[-1][0])

    def test_approved_permission_clears_when_its_call_completes(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.permission()
        self.assertEqual(self.field("--state"), "blocked")
        self.assertEqual(self.field("--reason"), "Permission required")
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_parallel_completion_keeps_the_approval_open(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.hook("PreToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.permission()
        self.hook("PostToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_a_call_started_after_the_approval_does_not_hold_it(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.permission()
        self.hook("PreToolUse", tool_use_id="later", tool_name="Bash", tool_input={"command": "ls"})
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_network_approval_for_the_same_command_needs_one_completion(self):
        # Codex reports a running command's network access as a second Bash approval carrying
        # the same command and a `network-access` description.
        self.hook("SessionStart")
        curl = {"tool_name": "Bash", "tool_input": {"command": "curl https://example.com"}}
        self.hook("PreToolUse", tool_use_id="exec-1", **curl)
        self.permission(**curl)
        self.hook("PermissionRequest", tool_name="Bash", tool_input={
            "command": "curl https://example.com", "description": "network-access example.com"})
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="exec-1", **curl)
        self.assertEqual(self.field("--state"), "working")

    def test_mcp_arguments_named_description_are_not_special(self):
        self.hook("SessionStart")
        deploy = {"tool_name": "mcp__ops__deploy",
                  "tool_input": {"description": "production deployment", "target": "foo"}}
        self.hook("PreToolUse", tool_use_id="mcp-1", **deploy)
        self.hook("PermissionRequest", **deploy)
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="mcp-1", **deploy)
        self.assertEqual(self.field("--state"), "working")

    def test_identical_parallel_calls_need_both_completions(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.hook("PreToolUse", tool_use_id="exec-2", **BASH)
        self.permission()
        self.permission()
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="exec-2", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_approval_without_a_running_call_stays_until_the_turn_ends(self):
        self.hook("SessionStart")
        self.hook("UserPromptSubmit")
        self.hook("PermissionRequest", tool_name="Bash", tool_input={"command": "ls"})
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("Stop")
        self.assertEqual(self.field("--state"), "done")

    def test_write_stdin_approval_waits_for_its_exec_call_across_turns(self):
        # Codex fires no PreToolUse for write_stdin, and the command it writes to can outlive the
        # turn that started it, so only its approval's parent_call_id identifies the call.
        self.hook("SessionStart")
        self.hook("UserPromptSubmit")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.hook("Stop")
        self.hook("UserPromptSubmit")
        self.hook("PermissionRequest", tool_name="write_stdin", tool_input={
            "chars": "y", "parent_call_id": "exec-1", "approval_id": "approval-1"})
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PreToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.hook("PostToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_write_stdin_approval_ignores_other_running_calls(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.hook("PreToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.hook("PermissionRequest", tool_name="write_stdin", tool_input={
            "chars": "y", "parent_call_id": "exec-2"})
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_use_id="exec-2", tool_name="Bash", tool_input={"command": "ls"})
        self.assertEqual(self.field("--state"), "working")

    def test_denied_permission_clears_when_the_turn_ends(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_use_id="exec-1", **BASH)
        self.permission()
        self.hook("Stop")
        self.assertEqual(self.field("--state"), "done")
        self.hook("UserPromptSubmit")
        self.hook("PostToolUse", tool_use_id="exec-1", **BASH)
        self.assertEqual(self.field("--state"), "working")

    def test_nested_codex_in_a_tool_shell_cannot_claim_the_pane(self):
        self.hook("SessionStart")
        self.env["CODEX_THREAD_ID"] = "thread-id"
        for name in ("SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"):
            self.hook(name, session="nested")
        self.assertEqual(len(self.calls), 1)
        self.hook("Stop")
        self.assertEqual(self.field("--state"), "done")

    def test_question_tool_blocks_until_its_own_completion(self):
        self.hook("SessionStart")
        self.hook("PreToolUse", tool_name="request_user_input", tool_use_id="question", tool_input={})
        self.assertEqual(self.field("--state"), "blocked")
        self.assertEqual(self.field("--reason"), "Question needs an answer")
        self.hook("PostToolUse", tool_name="Bash", tool_use_id="other", tool_input={})
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_name="request_user_input", tool_use_id="question", tool_input={})
        self.assertEqual(self.field("--state"), "working")

    def test_missing_endpoint_or_plugin_data_does_not_touch_disk(self):
        for key in ("ROZI", "ROZI_PANE", "ROZI_SESSION_INSTANCE", "PLUGIN_DATA"):
            env = self.env.copy()
            env.pop(key)
            activity.handle({"hook_event_name": "SessionStart", "session_id": "id"}, env)
        self.assertEqual(self.calls, [])
        self.assertEqual(list(Path(self.temp.name).iterdir()), [])

    def test_pane_without_ui_endpoint_reports_to_its_own_session(self):
        # A remote pane, or one in a session no window shows, has no ROZI_SOCKET. rozi sends
        # its report to the session server ROZI_SESSION_INSTANCE names, so the hook still reports.
        self.env.pop("ROZI_SOCKET")
        self.hook("SessionStart")
        args, kwargs = self.calls[-1]
        self.assertEqual(args[1:3], ["agents", "report"])
        self.assertNotIn("--session", args)
        self.assertNotIn("--target", args)
        self.assertNotIn("ROZI_SOCKET", kwargs["env"])
        self.assertEqual(kwargs["env"]["ROZI_SESSION_INSTANCE"], "session-instance")

    def test_invalid_thread_id_cannot_claim_pane(self):
        self.hook("SessionStart", session=None)
        self.hook("UserPromptSubmit", session="")
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

    def test_prompts_and_tool_input_are_not_reported(self):
        self.hook("SessionStart")
        self.hook("UserPromptSubmit", prompt="secret prompt")
        self.permission(tool_input={"command": "echo secret-command"})
        self.assertNotIn("secret", str(self.calls))

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
                list(pool.map(lambda index: self.hook("PostToolUse", tool_use_id=str(index), **BASH),
                              range(4)))
        # Concurrent snapshots can be coalesced, but every delivered sequence advances and the
        # final report includes all four committed events.
        seqs = [int(self.field("--seq", i)) for i in range(len(self.calls))]
        self.assertEqual(seqs, sorted(set(seqs)))
        self.assertEqual(seqs[-1], 5)

    def test_twenty_parallel_hooks_preserve_waits_and_deliver_latest_state(self):
        self.hook("SessionStart")
        waiting = {"tool_name": "Bash", "tool_input": {"command": "waiting"}}
        self.hook("PreToolUse", tool_use_id="waiting", **waiting)
        events = [
            ("PermissionRequest", waiting),
            ("PreToolUse", {"tool_name": "request_user_input", "tool_use_id": "question"}),
        ] + [("PostToolUse", {"tool_use_id": f"other-{index}", **BASH}) for index in range(18)]
        barrier = threading.Barrier(len(events))
        original = self.record

        def slow_record(args, **kwargs):
            time.sleep(0.01)
            return original(args, **kwargs)

        def run(event):
            barrier.wait(timeout=10)
            self.hook(event[0], **event[1])

        # This stress test checks concurrent state preservation, not hook latency. Allow slow
        # CI disks to serialize twenty SQLite writers; separate tests cover production deadlines.
        with patch.object(activity, "STATE_RETRY_BUDGET", 30.0), \
             patch.object(activity.subprocess, "run", side_effect=slow_record):
            with concurrent.futures.ThreadPoolExecutor(max_workers=len(events)) as pool:
                list(pool.map(run, events))
        self.assertEqual(self.field("--state"), "blocked")
        self.assertEqual(self.field("--seq"), "22")
        db = activity.open_state(activity.state_path(self.env))
        try:
            waits = db.execute("""SELECT (SELECT COUNT(*) FROM questions)
                + (SELECT COUNT(*) FROM approvals)""").fetchone()[0]
            self.assertEqual(waits, 2)
            self.assertEqual(db.execute("SELECT dirty FROM status").fetchone()[0], 0)
        finally:
            db.close()

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
                sender = pool.submit(self.hook, "PostToolUse", tool_use_id="other", **BASH)
                self.assertTrue(sending.wait(timeout=1))
                try:
                    pool.submit(self.permission).result(timeout=1)
                    db = activity.open_state(activity.state_path(self.env))
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
        path = activity.state_path(self.env)
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
                    permission = pool.submit(self.permission)
                    try:
                        self.assertTrue(retrying.wait(timeout=1))
                        # Exercise a real SQLite lock beyond one second.
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
            self.permission()
        db = activity.open_state(activity.state_path(self.env))
        try:
            self.assertEqual(db.execute("SELECT COUNT(*) FROM approvals").fetchone()[0], 1)
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

    def test_interrupt_retries_within_codex_three_second_limit(self):
        error = activity.sqlite3.OperationalError("database is locked")
        with patch.object(activity, "persist_once", side_effect=error) as persist, \
             patch.object(activity.time, "monotonic", side_effect=[10, 11.3]):
            with self.assertRaises(activity.sqlite3.OperationalError):
                self.hook("Interrupt")
        self.assertEqual(persist.call_count, 1)
        worst_case = (activity.SHORT_HOOK_BUDGETS["Interrupt"] + activity.DELIVERY_BUDGET
                      + activity.DELIVERY_LOCK_TIMEOUT + 2 * activity.STATE_LOCK_TIMEOUT)
        self.assertLess(worst_case + 0.3, 3)

    def test_non_lock_database_failure_is_not_retried(self):
        with patch.object(activity, "persist_once", side_effect=activity.sqlite3.OperationalError("disk I/O error")) as persist:
            with self.assertRaises(activity.sqlite3.OperationalError):
                self.hook("SessionStart")
        self.assertEqual(persist.call_count, 1)

    def test_switch_retries_release_after_timeout_launch_failure_or_nonzero_exit(self):
        failures = (
            subprocess.TimeoutExpired("rozi", 0.75), FileNotFoundError("rozi"),
            subprocess.CompletedProcess([], 1, stderr="session unavailable\n"),
        )
        for index, failure in enumerate(failures):
            self.env["ROZI_PANE"] = str(600 + index)
            self.hook("SessionStart")
            old_token = self.field("--integration")
            with patch.object(activity.subprocess, "run", side_effect=[failure]):
                if isinstance(failure, Exception):
                    with self.assertRaises(type(failure)):
                        self.hook("SessionStart", session="new-id", source="resume")
                else:
                    self.hook("SessionStart", session="new-id", source="resume")
            # New-thread events still belong to the new lifecycle and retry the old release.
            self.hook("PermissionRequest", session="new-id", **BASH)
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
        with patch.object(activity.subprocess, "run", side_effect=[refused, subprocess.CompletedProcess([], 0)]):
            self.hook("UserPromptSubmit", session="new-id")
        db = activity.open_state(activity.state_path(self.env))
        try:
            self.assertEqual(db.execute("SELECT session FROM run").fetchone()[0], "new-id")
            self.assertEqual(db.execute("SELECT COUNT(*) FROM releases").fetchone()[0], 0)
            self.assertEqual(db.execute("SELECT dirty FROM status").fetchone()[0], 0)
        finally:
            db.close()

    def test_retry_of_a_timed_out_report_uses_a_newer_sequence(self):
        self.hook("SessionStart")
        with patch.object(activity.subprocess, "run", side_effect=subprocess.TimeoutExpired("rozi", 0.75)):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.hook("UserPromptSubmit")
        path = activity.state_path(self.env)
        db = activity.open_state(path)
        try:
            activity.flush_pending(db, path, self.env)
        finally:
            db.close()
        self.assertEqual(self.field("--seq"), "3")
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

    def test_successive_clients_in_one_pane_share_state_but_not_tokens(self):
        # A client that exited without releasing, such as one that crashed, is released by the
        # next client's first SessionStart in the same pane.
        self.hook("SessionStart")
        crashed = self.field("--integration")
        self.hook("SessionStart", session="next-client")
        self.assertEqual(self.calls[-2][0][1:3], ["agents", "release"])
        self.assertEqual(self.field("--integration", -2), crashed)
        self.assertNotEqual(self.field("--integration"), crashed)
        self.assertEqual(len(list(Path(self.temp.name).rglob("*.sqlite3"))), 1)

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

    def test_manifest_hooks_run_synchronously_within_codex_limits(self):
        manifest = json.loads((ROOT / ".codex-plugin/plugin.json").read_text())
        self.assertEqual(manifest["hooks"], "./hooks/hooks.json")
        hooks = json.loads((ROOT / "hooks/hooks.json").read_text())["hooks"]
        self.assertEqual(set(hooks), {"SessionStart", "UserPromptSubmit", "PreToolUse",
                                      "PermissionRequest", "PostToolUse", "PreCompact",
                                      "PostCompact", "Stop", "Interrupt", "SessionEnd"})
        self.assertLess(activity.CLI_TIMEOUT, activity.DELIVERY_BUDGET)
        self.assertLess(activity.DELIVERY_BUDGET, activity.STATE_RETRY_BUDGET)
        for name, groups in hooks.items():
            self.assertEqual(len(groups), 1, name)
            self.assertNotIn("matcher", groups[0], name)
            hook = groups[0]["hooks"][0]
            self.assertEqual(hook["type"], "command")
            self.assertEqual(hook["command"], 'python3 "${PLUGIN_ROOT}/scripts/activity.py"')
            self.assertFalse(hook.get("async", False), name)
            budget = activity.SHORT_HOOK_BUDGETS.get(name, activity.STATE_RETRY_BUDGET)
            implementation_budget = (budget + activity.DELIVERY_BUDGET
                                     + activity.DELIVERY_LOCK_TIMEOUT + 2 * activity.STATE_LOCK_TIMEOUT)
            self.assertGreater(hook["timeout"], implementation_budget + 0.5, name)
        self.assertEqual(hooks["Interrupt"][0]["hooks"][0]["timeout"], 3)


@unittest.skipIf(os.name == "nt" or not shutil.which("sh") or not shutil.which("python3"),
                 "needs a POSIX shell and python3")
class ShellLaunchTests(unittest.TestCase):
    """Runs every hook the way Codex does: the manifest's command line, through a shell, as its own
    process. The trailing `; exit` keeps the shell from replacing itself with Python, as fish,
    some `/bin/sh` builds, and `cmd.exe` do not, so each hook has a different parent process."""

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="rozi codex shell ")
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.log = root / "rozi calls.log"
        fake = root / "rozi"
        fake.write_text(f'#!/bin/sh\nprintf "%s\\n" "$*" >> "{self.log}"\n')
        fake.chmod(0o755)
        hooks = json.loads((ROOT / "hooks/hooks.json").read_text())["hooks"]
        self.commands = {name: groups[0]["hooks"][0]["command"] for name, groups in hooks.items()}
        self.env = {
            "PATH": os.environ["PATH"], "PLUGIN_ROOT": str(ROOT),
            "PLUGIN_DATA": str(root / "data"), "ROZI": "1", "ROZI_PANE": "4",
            "ROZI_SOCKET": "ui-endpoint", "ROZI_SESSION_INSTANCE": "session-instance",
            "ROZI_BIN": str(fake), "PYTHONDONTWRITEBYTECODE": "1",
        }
        (root / "data").mkdir()

    def run_hook(self, name, **extra):
        event = {"hook_event_name": name, "session_id": "thread-id", **extra}
        result = subprocess.run(["sh", "-c", self.commands[name] + "; exit $?"],
                                input=json.dumps(event), env=self.env, capture_output=True,
                                text=True, timeout=10, check=False)
        self.assertEqual((result.returncode, result.stdout, result.stderr), (0, "", ""), name)

    def test_lifecycle_survives_a_new_parent_process_for_every_hook(self):
        call = {"tool_use_id": "exec-1", **BASH}
        self.run_hook("SessionStart", source="startup")
        self.run_hook("UserPromptSubmit", prompt="test")
        self.run_hook("PreToolUse", **call)
        self.run_hook("PermissionRequest", tool_name="Bash", tool_input={
            **BASH["tool_input"], "description": "Do you approve running cargo test?"})
        self.run_hook("PostToolUse", **call)
        self.run_hook("Stop", stop_hook_active=False)
        self.run_hook("SessionEnd", reason="other")
        calls = [line.split() for line in self.log.read_text().splitlines()]
        states = [args[args.index("--state") + 1] if "--state" in args else args[1]
                  for args in calls]
        self.assertEqual(states, ["idle", "working", "working", "blocked", "working", "done",
                                  "release"])
        tokens = {args[args.index("--integration") + 1] for args in calls}
        self.assertEqual(len(tokens), 1)


if __name__ == "__main__":
    unittest.main()
