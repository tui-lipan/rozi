#!/usr/bin/env python3
"""Paced Kitty image patches for a real PTY performance comparison.

Run inside a Rozi pane. Pixels are generated locally and immutable named files
keep each transmitted frame stable for composed and native host rendering. SIGWINCH
rebuilds the scene at the new pixel size. Use --full for full-frame traffic.
"""
import argparse
import base64
from collections import deque
import fcntl
import json
import os
from pathlib import Path
import signal
import struct
import tempfile
import termios
import time


class FrameFiles:
    """Retain immutable transfers for a grace period within a fixed byte budget."""

    def __init__(self, root, retain_seconds, max_bytes):
        self.root = root
        self.retain_seconds = retain_seconds
        self.max_bytes = max_bytes
        self.pending = deque()
        self.bytes = 0
        self.sequence = 0

    def send(self, pixels, image_id, z, size, position=(0, 0)):
        if len(pixels) > self.max_bytes:
            raise ValueError("one transfer exceeds --max-file-mib")
        while self.pending:
            path, sent, length = self.pending[0]
            remaining = sent + self.retain_seconds - time.monotonic()
            if remaining > 0:
                if self.bytes + len(pixels) <= self.max_bytes:
                    break
                # Slow the producer instead of deleting files still in their grace period.
                time.sleep(remaining)
                continue
            path.unlink()
            self.pending.popleft()
            self.bytes -= length
        path = self.root / f"frame-{self.sequence}.rgba"
        self.sequence += 1
        with path.open("xb") as output:
            output.write(pixels)
        transmit(path, image_id, z, size, position)
        self.pending.append((path, time.monotonic(), len(pixels)))
        self.bytes += len(pixels)

    def drain(self):
        if self.pending:
            time.sleep(max(0, self.pending[-1][1] + self.retain_seconds - time.monotonic()))


def viewport():
    rows, cols, width, height = struct.unpack("HHHH", fcntl.ioctl(1, termios.TIOCGWINSZ, bytes(8)))
    return rows, cols, width, height


def transmit(path, image_id, z, size, position=(0, 0)):
    rows, cols, width, height = viewport()
    cell_w, cell_h = width // max(cols, 1), height // max(rows, 1)
    x, y = position
    col, px = divmod(x, max(cell_w, 1))
    row, py = divmod(y, max(cell_h, 1))
    name = base64.b64encode(os.fsencode(path)).decode()
    sequence = (f"\x1b[{row + 1};{col + 1}H"
                f"\x1b_Ga=T,f=32,t=f,i={image_id},s={size[0]},v={size[1]},"
                f"X={px},Y={py},z={z},C=1,q=2;{name}\x1b\\")
    os.write(1, sequence.encode())


def background(width, height):
    pixels = bytearray()
    for y in range(height):
        row = bytes(channel for x in range(width)
                    for channel in (30 + (x // 37 % 4) * 25,
                                    30 + (y // 29 % 4) * 25, 130, 255))
        pixels.extend(row)
    return pixels


def build_scene(files, size):
    rows, cols, width, height = size
    os.write(1, b"\x1b[?25l\x1b[2J\x1b[H\x1b_Ga=d,d=A,q=2;\x1b\\")
    base = background(width, height)
    files.send(base, 1, 0, (width, height))
    toolbar_height = max(1, height // rows)
    files.send(bytes((40, 170, 170, 255)) * width * toolbar_height,
               4, 1, (width, toolbar_height))
    return base


def patch_frame(files, size, frame):
    rows, cols, width, height = size
    cell_w, cell_h = width // cols, height // rows
    color = (240, 40 + frame % 4 * 40, 40, 255)
    files.send(bytes(color) * 37 * 23, 2, 2, (37, 23),
               (cell_w * 3 + 3, cell_h * 3 + 5))
    position = (cell_w * (7 + frame // 10 % max(1, cols - 18)) + 7, cell_h * 6 + 9)
    files.send(bytes((40, 240, 80, 160)) * 127 * 75, 3, 3, (127, 75), position)


def full_frame(files, size, base, frame):
    pixels = bytearray(base)
    width = size[2]
    color = bytes((240, 40 + frame % 4 * 40, 40, 255)) * 37
    for y in range(65, min(88, size[3])):
        start = (y * width + 37) * 4
        pixels[start:start + len(color)] = color
    files.send(pixels, 1, 0, (width, size[3]))


def run(options):
    with tempfile.TemporaryDirectory(prefix="rozi-graphics-") as directory:
        root = Path(directory)
        files = FrameFiles(root, options.retain_seconds, options.max_file_mib * 1024 * 1024)
        previous = None
        resized = [True]
        signal.signal(signal.SIGWINCH, lambda *_: resized.__setitem__(0, True))
        started = time.monotonic()
        frame = 0
        try:
            while time.monotonic() - started < options.duration:
                size = viewport()
                if min(size) == 0:
                    time.sleep(0.05)
                    continue
                if size != previous or resized[0]:
                    resized[0] = False
                    base = build_scene(files, size)
                    previous = size
                if options.full:
                    full_frame(files, size, base, frame)
                else:
                    patch_frame(files, size, frame)
                if options.status:
                    options.status.write_text(json.dumps({"pid": os.getpid(), "frame": frame,
                                                          "size": size, "files": str(root)}))
                frame += 1
                time.sleep(max(0, started + frame / options.fps - time.monotonic()))
        finally:
            files.drain()
            os.write(1, b"\x1b_Ga=d,d=A,q=2;\x1b\\\x1b[?25h\x1b[Hdone\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fps", type=int, default=20)
    parser.add_argument("--duration", type=float, default=300)
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--status", type=Path)
    parser.add_argument("--retain-seconds", type=float, default=2,
                        help="minimum file lifetime after transmission (default: 2 seconds)")
    parser.add_argument("--max-file-mib", type=int, default=256,
                        help="retained file budget; throttle when full (default: 256 MiB)")
    options = parser.parse_args()
    if options.fps <= 0:
        parser.error("--fps must be positive")
    if options.retain_seconds <= 0 or options.max_file_mib <= 0:
        parser.error("--retain-seconds and --max-file-mib must be positive")
    run(options)


if __name__ == "__main__":
    main()
