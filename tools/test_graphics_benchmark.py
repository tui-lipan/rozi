import importlib.util
from pathlib import Path
import struct
import tempfile
import unittest


spec = importlib.util.spec_from_file_location("graphics_benchmark", Path(__file__).with_name("graphics-benchmark.py"))
benchmark = importlib.util.module_from_spec(spec)
spec.loader.exec_module(benchmark)


class WireMeasurements(unittest.TestCase):
    def summary(self, records, start=2000, end=5000):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "wire.bin"
            path.write_bytes(b"".join(struct.pack("=QI", timestamp, len(data)) + data
                                     for timestamp, data in records))
            return benchmark.wire_summary(path, start, end)

    def test_split_frame_and_graphics_commands_are_counted_at_completion(self):
        report = self.summary([
            (1000, b"\x1b_Ga=t,i=7;"), (2000, b"YWJj\x1b\\\x1b[?202"),
            (3000, b"6l"), (4000, b"\x1b_Ga=p,i=7,p=9;\x1b\\\x1b[?2026l"),
            (6000, b"\x1b_Ga=d,d=I,i=7;\x1b\\\x1b[?2026l")])
        self.assertEqual(report["uploads"], 1)
        self.assertEqual(report["placements"], 1)
        self.assertEqual(report["deletes"], 0)
        self.assertEqual(report["synchronized_frame_ends"], 2)
        self.assertEqual(report["synchronized_frame_interval_us"]["p50"], 1)

    def test_intervals_longer_than_two_frames_are_counted(self):
        frames = [(timestamp, b"\x1b[?2026l") for timestamp in [2000, 2010, 2080, 2090, 2160]]
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "wire.bin"
            path.write_bytes(b"".join(struct.pack("=QI", timestamp, len(data)) + data
                                     for timestamp, data in frames))
            report = benchmark.wire_summary(path, 0, 5000, frame_period_us=0.01)
        self.assertEqual(report["intervals_over_two_frames"], 2)
        self.assertEqual(report["first_long_interval_offset_s"], 2010 / 1e9)

    def test_truncated_record_cannot_silently_undercount_output(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "wire.bin"
            for data in [b"short", struct.pack("=QI", 1000, 8) + b"short"]:
                path.write_bytes(data)
                with self.assertRaises(ValueError):
                    benchmark.wire_summary(path, 0, 2000)

    def test_empty_capture_has_no_claimed_frame_percentiles(self):
        report = self.summary([])
        self.assertEqual(report["synchronized_frame_ends"], 0)
        self.assertIsNone(report["synchronized_frame_interval_us"])


if __name__ == "__main__":
    unittest.main()
