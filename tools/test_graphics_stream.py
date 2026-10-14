"""Fast fixture and producer clock checks; no PTY or subprocess required."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("graphics_stream", Path(__file__).with_name("graphics-stream.py"))
stream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(stream)


class GraphicsStreamTests(unittest.TestCase):
    def test_fixture_pool_is_bounded_and_each_frame_is_immutable(self):
        with tempfile.TemporaryDirectory() as directory:
            packets, used = stream.fixture_pool(Path(directory), 16, 8, "moving", 2048)
            files = sorted(Path(directory).glob("*.rgba"))
            before = [path.read_bytes() for path in files]
            self.assertEqual(len(packets), 4)
            self.assertEqual(used, 2048)
            self.assertTrue(all(len(data) == 512 for data in before))
            self.assertTrue(all(b"i=7,p=9," in packet for packet in packets))
            self.assertTrue(all(packet.startswith(b"\x1b_G") for packet in packets))
            self.assertEqual(before, [path.read_bytes() for path in files])
            with self.assertRaises(ValueError):
                stream.fixture_pool(Path(directory), 16, 8, "whole", 511)

    def test_fixtures_carry_their_pool_index_in_pixel_zero(self):
        with tempfile.TemporaryDirectory() as directory:
            stream.fixture_pool(Path(directory), 16, 8, "moving", 2048)
            for frame in range(4):
                data = (Path(directory) / f"16x8-moving-{frame}.rgba").read_bytes()
                self.assertEqual(data[:4], bytes((frame, 0)) + stream.FIXTURE_MARK)

    def test_unchanged_mode_reuses_one_file(self):
        with tempfile.TemporaryDirectory() as directory:
            packets, used = stream.fixture_pool(Path(directory), 16, 8, "unchanged", 4096)
            self.assertEqual((len(packets), used), (1, 512))

    def test_late_writes_skip_slots_without_losing_clock_phase(self):
        for fps in [30, 60, 120, 144, 240]:
            interval = 1 / fps
            next_deadline = stream.advance_deadline(0, interval * 3.5, interval)
            self.assertAlmostEqual(next_deadline, interval * 4)
            self.assertAlmostEqual(stream.advance_deadline(next_deadline, next_deadline, interval), interval * 5)


if __name__ == "__main__":
    unittest.main()
