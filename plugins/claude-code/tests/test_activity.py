"""Hook lifecycle tests with isolated state and a recorded public Rozi CLI."""

import concurrent.futures
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tempfile
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
        for name in ("SessionStart", "UserPromptSubmit", "PermissionRequest", "PostToolUse", "Stop", "SessionEnd"):
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
        self.hook("PreToolUse", tool_name="AskUserQuestion")
        self.assertEqual(self.field("--state"), "blocked")
        self.hook("PostToolUse", tool_name="AskUserQuestion")
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
        self.assertEqual(kwargs["timeout"], activity.CLI_TIMEOUT)
        self.assertEqual(kwargs["stdout"], subprocess.DEVNULL)
        self.assertEqual(kwargs["stderr"], subprocess.DEVNULL)

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
        self.assertEqual([self.field("--seq", i) for i in range(5)], ["1", "2", "3", "4", "5"])

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
        for name, groups in hooks.items():
            hook = groups[0]["hooks"][0]
            self.assertEqual(hook["command"], "python")
            self.assertEqual(hook["args"], ["${CLAUDE_PLUGIN_ROOT}/scripts/activity.py"])
            self.assertFalse(hook.get("async", False), name)
            self.assertGreater(hook["timeout"], activity.CLI_TIMEOUT)
        self.assertNotIn("SubagentStop", hooks)


if __name__ == "__main__":
    unittest.main()
