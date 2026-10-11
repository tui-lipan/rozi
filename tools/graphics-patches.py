#!/usr/bin/env python3
"""Paced Kitty image patches for a real PTY performance comparison.

Run inside a Rozi pane. Pixels are generated locally and reusable file transport
keeps the input identical for composed and native host rendering. SIGWINCH
rebuilds the scene at the new pixel size. Use --full for full-frame traffic.
"""
import argparse
import base64
import fcntl
import json
import os
from pathlib import Path
import signal
import struct
import tempfile
import termios
import time


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


def build_scene(root, size):
    rows, cols, width, height = size
    os.write(1, b"\x1b[?25l\x1b[2J\x1b[H\x1b_Ga=d,d=A,q=2;\x1b\\")
    base = background(width, height)
    (root / "base.rgba").write_bytes(base)
    transmit(root / "base.rgba", 1, 0, (width, height))
    toolbar_height = max(1, height // rows)
    (root / "toolbar.rgba").write_bytes(bytes((40, 170, 170, 255)) * width * toolbar_height)
    transmit(root / "toolbar.rgba", 4, 1, (width, toolbar_height))
    return base


def patch_frame(root, size, frame):
    rows, cols, width, height = size
    cell_w, cell_h = width // cols, height // rows
    color = (240, 40 + frame % 4 * 40, 40, 255)
    patch = root / "small.rgba"
    patch.write_bytes(bytes(color) * 37 * 23)
    transmit(patch, 2, 2, (37, 23), (cell_w * 3 + 3, cell_h * 3 + 5))
    patch = root / "moving.rgba"
    patch.write_bytes(bytes((40, 240, 80, 160)) * 127 * 75)
    position = (cell_w * (7 + frame // 10 % max(1, cols - 18)) + 7, cell_h * 6 + 9)
    transmit(patch, 3, 3, (127, 75), position)


def full_frame(root, size, base, frame):
    pixels = bytearray(base)
    width = size[2]
    color = bytes((240, 40 + frame % 4 * 40, 40, 255)) * 37
    for y in range(65, min(88, size[3])):
        start = (y * width + 37) * 4
        pixels[start:start + len(color)] = color
    (root / "full.rgba").write_bytes(pixels)
    transmit(root / "full.rgba", 1, 0, (width, size[3]))


def run(options):
    with tempfile.TemporaryDirectory(prefix="rozi-graphics-") as directory:
        root = Path(directory)
        previous = None
        resized = [True]
        signal.signal(signal.SIGWINCH, lambda *_: resized.__setitem__(0, True))
        started = time.monotonic()
        frame = 0
        while time.monotonic() - started < options.duration:
            size = viewport()
            if min(size) == 0:
                time.sleep(0.05)
                continue
            if size != previous or resized[0]:
                resized[0] = False
                base = build_scene(root, size)
                previous = size
            if options.full:
                full_frame(root, size, base, frame)
            else:
                patch_frame(root, size, frame)
            if options.status:
                options.status.write_text(json.dumps({"pid": os.getpid(), "frame": frame,
                                                      "size": size, "files": str(root)}))
            frame += 1
            time.sleep(max(0, started + frame / options.fps - time.monotonic()))
        os.write(1, b"\x1b_Ga=d,d=A,q=2;\x1b\\\x1b[?25h\x1b[Hdone\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fps", type=int, default=20)
    parser.add_argument("--duration", type=float, default=300)
    parser.add_argument("--full", action="store_true")
    parser.add_argument("--status", type=Path)
    options = parser.parse_args()
    if options.fps <= 0:
        parser.error("--fps must be positive")
    run(options)


if __name__ == "__main__":
    main()
