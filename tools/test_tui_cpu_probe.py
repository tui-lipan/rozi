"""Checks that sidebar CPU samples cannot silently measure the wrong UI state."""
import argparse
import importlib.util
import json
from pathlib import Path
import unittest

spec = importlib.util.spec_from_file_location(
    "tui_cpu_probe", Path(__file__).with_name("tui-cpu-probe.py"))
probe = importlib.util.module_from_spec(spec)
spec.loader.exec_module(probe)


class SidebarProbeTests(unittest.TestCase):
    def test_original_workload_matrix_is_preserved(self):
        self.assertEqual([mode for mode, _, _ in probe.cases(False)],
                         ["idle", "body", "both", "title", "body", "dense"])

    def test_sidebar_controls_repeat_both_static_and_streaming_cases(self):
        cases = probe.cases(True)
        self.assertEqual(cases[0], cases[4])
        self.assertEqual(cases[1], cases[3])
        self.assertEqual(cases[5], cases[7])
        self.assertEqual(cases[2], ("idle", True, "idle"))
        self.assertEqual(cases[6], ("body", True, "working"))

    def test_capture_checks_visibility_and_actual_animation(self):
        probe.verify_sidebar("Activity\n⠹ Synthetic work", True, "working")
        probe.verify_sidebar("Activity\n○ Synthetic work", True, "idle")
        probe.verify_sidebar("rozi\nbody", False, "working")
        for text, visible, status in [
            ("", True, "working"),
            ("rozi\nbody", True, "working"),
            ("Activity\n○ Synthetic work", True, "working"),
            ("Activity\n⠹ Synthetic work", True, "idle"),
            ("Activity\nbody", False, "working"),
        ]:
            with self.subTest(text=text, visible=visible, status=status):
                with self.assertRaises(RuntimeError):
                    probe.verify_sidebar(text, visible, status)

    def test_viewports_reject_malformed_and_too_small_sizes(self):
        self.assertEqual(probe.parse_viewport("475x116"), (475, 116))
        for value in ["", "80", "80x", "0x24", "80x0", "79x24", "80x23", "axt", "80x24x3"]:
            with self.subTest(value=value), self.assertRaises(argparse.ArgumentTypeError):
                probe.parse_viewport(value)

    def test_pane_setup_counts_actual_panes_including_an_empty_server(self):
        for initial in [0, 1, 2]:
            panes = [{"id": i} for i in range(initial)]
            splits = []

            def command(*args):
                if "split" in args:
                    splits.append(args)
                    panes.append({"id": len(panes)})
                return json.dumps({"data": panes})

            probe.populate_panes(command, "owned-probe", 2, ["python", "work.py"])
            self.assertEqual(len(panes), 2)
            self.assertEqual(len(splits), 2 - initial)


if __name__ == "__main__":
    unittest.main()
