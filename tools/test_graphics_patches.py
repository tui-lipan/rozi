"""Regression checks for delayed readers of the graphics workload."""
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location(
    "graphics_patches", Path(__file__).with_name("graphics-patches.py"))
workload = importlib.util.module_from_spec(spec)
spec.loader.exec_module(workload)


class FrameFilesTests(unittest.TestCase):
    def test_delayed_readers_get_each_original_frame_including_after_resize(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(workload, "transmit") as send, \
                patch.object(workload.os, "write"):
            files = workload.FrameFiles(Path(directory), 10, 1024 * 1024)
            size = (12, 40, 400, 240)
            workload.patch_frame(files, size, 0)
            workload.patch_frame(files, size, 1)
            workload.build_scene(files, (4, 8, 80, 80))
            workload.build_scene(files, (4, 8, 88, 88))
            paths = [call.args[0] for call in send.call_args_list]
            self.assertEqual(len(paths), len(set(paths)))
            self.assertEqual(paths[0].read_bytes(), bytes((240, 40, 40, 255)) * 37 * 23)
            self.assertEqual(paths[2].read_bytes(), bytes((240, 80, 40, 255)) * 37 * 23)
            self.assertEqual(len(paths[4].read_bytes()), 80 * 80 * 4)
            self.assertEqual(len(paths[6].read_bytes()), 88 * 88 * 4)

    def test_budget_throttles_until_old_files_expire_without_reusing_names(self):
        now = [0.0]
        with tempfile.TemporaryDirectory() as directory, \
                patch.object(workload, "transmit") as send, \
                patch.object(workload.time, "monotonic", side_effect=lambda: now[0]), \
                patch.object(workload.time, "sleep", side_effect=lambda seconds: now.__setitem__(0, now[0] + seconds)) as sleep:
            files = workload.FrameFiles(Path(directory), 2, 8)
            files.send(b"12345678", 1, 0, (2, 1))
            first = send.call_args.args[0]
            files.send(b"abcdefgh", 1, 0, (2, 1))
            second = send.call_args.args[0]
            sleep.assert_called_once_with(2)
            self.assertFalse(first.exists())
            self.assertNotEqual(first, second)
            self.assertEqual(second.read_bytes(), b"abcdefgh")
            self.assertEqual(files.bytes, 8)
            files.drain()
            self.assertEqual(now[0], 4)

    def test_full_frames_remain_immutable_and_oversize_transfer_is_rejected(self):
        with tempfile.TemporaryDirectory() as directory, patch.object(workload, "transmit") as send, \
                patch.object(workload.os, "write"):
            files = workload.FrameFiles(Path(directory), 10, 1024 * 1024)
            size = (12, 40, 400, 240)
            base = bytes((30, 30, 130, 255)) * 400 * 240
            workload.full_frame(files, size, base, 0)
            first = send.call_args.args[0]
            original = first.read_bytes()
            workload.full_frame(files, size, base, 1)
            self.assertEqual(first.read_bytes(), original)
            self.assertNotEqual(send.call_args.args[0].read_bytes(), original)
            with self.assertRaisesRegex(ValueError, "exceeds"):
                files.send(bytes(files.max_bytes + 1), 1, 0, (1, 1))


if __name__ == "__main__":
    unittest.main()
