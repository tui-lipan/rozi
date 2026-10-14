#!/usr/bin/env python3
"""Immutable file-backed Kitty replacements for presentation and pacing measurements.

No files are rewritten or deleted while queued frames might still refer to them.
The bounded fixture pool is retained after exit; its path appears in the report.
Producer writes are measured separately from successful host presentations.
"""
import argparse
import base64
import fcntl
import json
import math
import os
from pathlib import Path
import select
import signal
import struct
import tempfile
import termios
import time


def write_all(fd, data):
    while data:
        written = os.write(fd, data)
        data = data[written:]


def viewport():
    rows, cols, width, height = struct.unpack("HHHH", fcntl.ioctl(1, termios.TIOCGWINSZ, bytes(8)))
    return rows, cols, width or cols * 10, height or rows * 20


def replacement(path, width, height):
    name = base64.b64encode(os.fsencode(path))
    return (f"\x1b_Ga=T,f=32,t=f,i=7,p=9,s={width},v={height},C=1,q=2;".encode()
            + name + b"\x1b\\")


def pixels(width, height, mode, frame):
    color = bytes((30 + frame * 7 % 180, 50 + frame * 13 % 180, 130, 255))
    if mode == "whole":
        return color * width * height
    result = bytearray(bytes((30, 60, 130, 255)) * width * height)
    if mode == "moving":
        rect_w, rect_h = min(48, width), min(32, height)
        left = frame * 17 % max(1, width - rect_w + 1)
        top = frame * 7 % max(1, height - rect_h + 1)
        for row in range(top, top + rect_h):
            start = (row * width + left) * 4
            result[start:start + rect_w * 4] = bytes((240, 60, 40, 255)) * rect_w
    return result


def fixture_pool(root, width, height, mode, budget):
    frame_bytes = width * height * 4
    count = min(1 if mode == "unchanged" else 64, budget // frame_bytes)
    if count < 1:
        raise ValueError("one frame exceeds the fixture byte budget")
    packets = []
    for frame in range(count):
        path = root / f"{width}x{height}-{mode}-{frame}.rgba"
        path.write_bytes(pixels(width, height, mode, frame))
        packets.append(replacement(path, width, height))
    return packets, count * frame_bytes


def advance_deadline(deadline, now, interval):
    return deadline + max(1, math.floor((now - deadline) / interval) + 1) * interval


def run(options):
    if options.files:
        options.files.mkdir(parents=True, exist_ok=True)
    root = Path(tempfile.mkdtemp(prefix="rozi-graphics-stream-", dir=options.files))
    size = viewport()
    width, height = options.width or size[2], options.height or size[3]
    packets, fixture_bytes = fixture_pool(root, width, height, options.mode,
                                          options.max_file_mib * 1024 * 1024)
    if options.ready_file:
        options.ready_file.write_text(json.dumps({"pid": os.getpid(), "files": str(root),
                                                  "fixture_bytes": fixture_bytes}) + "\n")
    resize = [False]
    signal.signal(signal.SIGWINCH, lambda *_: resize.__setitem__(0, True))
    started = time.monotonic()
    deadline = started
    interval = 1 / options.fps
    frames = wire_bytes = 0
    blocked_seconds = 0.0
    mixed_deadline = started + 1
    input_echoes = resize_events = 0
    write_all(1, b"\x1b[?25l\x1b[2J\x1b[H")
    try:
        while time.monotonic() - started < options.duration:
            if resize[0]:
                resize[0] = False
                resize_events += 1
                # Preserve immutable fixtures; host clipping changes with the viewport.
                write_all(1, b"\x1b[H\x1b_Ga=d,d=i,i=7,q=2;\x1b\\")
            now = time.monotonic()
            if options.mixed and now >= mixed_deadline:
                write_all(1, (f"\x1b]2;graphics frame {frames}\x07\r\nbarrier {frames}\r\n"
                              "\x1b_Ga=q,i=99,f=24,s=1,v=1;YWJj\x1b\\\x1b[H").encode())
                if frames and int(now - started) % 5 == 0:
                    write_all(1, b"\x1b_Ga=d,d=I,i=7,q=2;\x1b\\")
                mixed_deadline = now + 1
            if options.mixed and select.select([0], [], [], 0)[0]:
                incoming = os.read(0, 4096)
                if incoming:
                    write_all(1, b"\r\necho: " + incoming + b"\r\n\x1b[H")
                    input_echoes += 1
            packet = packets[frames % len(packets)]
            before = time.monotonic()
            write_all(1, packet)
            after = time.monotonic()
            blocked_seconds += after - before
            wire_bytes += len(packet)
            frames += 1
            deadline = advance_deadline(deadline, after, interval)
            time.sleep(max(0, deadline - time.monotonic()))
    finally:
        elapsed = time.monotonic() - started
        write_all(1, b"\x1b[?25h\r\nstream complete\r\n")
        report = dict(pid=os.getpid(), mode=options.mode, requested_fps=options.fps,
                      producer_frames=frames, producer_fps=frames / elapsed,
                      elapsed_seconds=elapsed, write_seconds=blocked_seconds,
                      kitty_bytes=wire_bytes, fixture_bytes=fixture_bytes,
                      fixture_files=len(packets), files=str(root), width=width, height=height,
                      input_echoes=input_echoes, resize_events=resize_events)
        if options.report:
            options.report.write_text(json.dumps(report, indent=2) + "\n")
        else:
            write_all(2, (json.dumps(report) + "\n").encode())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fps", type=int, choices=[30, 60, 120, 144, 240], default=120)
    parser.add_argument("--duration", type=float, default=10)
    parser.add_argument("--mode", choices=["unchanged", "moving", "whole"], default="moving")
    parser.add_argument("--mixed", action="store_true")
    parser.add_argument("--width", type=int)
    parser.add_argument("--height", type=int)
    parser.add_argument("--max-file-mib", type=int, default=128)
    parser.add_argument("--files", type=Path)
    parser.add_argument("--report", type=Path)
    parser.add_argument("--ready-file", type=Path)
    options = parser.parse_args()
    if options.duration <= 0 or options.max_file_mib <= 0 or any(
            value is not None and value <= 0 for value in [options.width, options.height]):
        parser.error("duration, dimensions and file budget must be positive")
    run(options)


if __name__ == "__main__":
    main()
