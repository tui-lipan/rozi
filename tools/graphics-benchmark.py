#!/usr/bin/env python3
"""Measure an owned Ghostty/Rozi graphics stream, with no builds during sampling.

Requires Linux /proc and the ghostty-window-lab helper on Hyprland. All runtime
and producer files stay in the specified private output directory. Producer
writes and successful host frames are reported separately.
"""
import argparse
import bisect
import hashlib
import json
import math
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import time


def run(argv, env=None):
    return subprocess.check_output(argv, env=env, text=True, timeout=20)


def wait_for(read, label):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        value = read()
        if value:
            return value
        time.sleep(.025)
    raise RuntimeError(f"timed out waiting for {label}")


def process_sample(pid):
    root = Path(f"/proc/{pid}")
    try:
        fields = (root / "stat").read_text().rsplit(")", 1)[1].split()
        rss = int((root / "statm").read_text().split()[1]) * os.sysconf("SC_PAGE_SIZE")
        pss = {line.split(":", 1)[0]: int(line.split()[1]) * 1024
               for line in (root / "smaps_rollup").read_text().splitlines()
               if line.startswith(("Pss:", "Pss_Anon:", "Pss_File:", "Pss_Shmem:"))}
        return {"pid": pid, "identity": fields[19],
                "ticks": int(fields[11]) + int(fields[12]), "rss_bytes": rss, "pss_bytes": pss}
    except (FileNotFoundError, ProcessLookupError):
        return None


def cpu_report(first, last, elapsed):
    if not first or not last or first["identity"] != last["identity"]:
        return None
    return {"pid": first["pid"],
            "cpu_percent_one_core": (last["ticks"] - first["ticks"]) * 100
            / os.sysconf("SC_CLK_TCK") / elapsed,
            "rss_before_bytes": first["rss_bytes"], "rss_after_bytes": last["rss_bytes"],
            "pss_before_bytes": first["pss_bytes"], "pss_after_bytes": last["pss_bytes"]}


def percentiles(values):
    if not values:
        return None
    ordered = sorted(values)
    return {f"p{percent}": ordered[max(0, math.ceil(len(ordered) * percent / 100) - 1)]
            for percent in [50, 95, 99]} | {"max": ordered[-1]}


def wire_records(path):
    data, offset = path.read_bytes(), 0
    while offset < len(data):
        if offset + 12 > len(data):
            raise ValueError("truncated wire record header")
        timestamp, length = struct.unpack_from("=QI", data, offset)
        offset += 12
        if offset + length > len(data):
            raise ValueError("truncated wire record payload")
        yield timestamp, data[offset:offset + length]
        offset += length


def wire_summary(path, started, ended, frame_period_us=None):
    wire = bytearray()
    ends, times = [], []
    sample_bytes = 0
    for timestamp, data in wire_records(path):
        wire.extend(data)
        ends.append(len(wire))
        times.append(timestamp)
        if started <= timestamp <= ended:
            sample_bytes += len(data)

    def in_sample(match):
        return started <= times[bisect.bisect_left(ends, match.end())] <= ended

    frame_times = [times[bisect.bisect_left(ends, match.end())]
                   for match in re.finditer(rb"\x1b\[\?2026l", wire) if in_sample(match)]
    commands = [match for match in re.finditer(rb"\x1b_G[^\x1b]*\x1b\\", wire) if in_sample(match)]
    headers = [match.group().split(b";", 1)[0] for match in commands]
    intervals = [(right - left) / 1000 for left, right in zip(frame_times, frame_times[1:])]
    # Percentiles hide a stall that recurs in only part of the sample; count each one.
    stalls = [(index, interval) for index, interval in enumerate(intervals)
              if frame_period_us and interval > 2 * frame_period_us]
    return {"host_bytes": sample_bytes, "kitty_commands": len(commands),
            "uploads": sum(b"a=t," in header or b"a=T," in header for header in headers),
            "placements": sum(b"a=p," in header for header in headers),
            "deletes": sum(b"a=d," in header for header in headers),
            "synchronized_frame_ends": len(frame_times),
            "synchronized_frame_interval_us": percentiles(intervals),
            "intervals_over_two_frames": len(stalls),
            "first_long_interval_offset_s": (frame_times[stalls[0][0]] - started) / 1e9
            if stalls else None}


def media_marks(path):
    """Yield (monotonic ns, fixture index) for each upload whose pixel 0 carries a fixture mark."""
    data = path.read_bytes()
    if len(data) % 16:
        raise ValueError("truncated media record")
    for offset in range(0, len(data), 16):
        timestamp, pixel, read = struct.unpack_from("=Q4sI", data, offset)
        if read and pixel[2:] == bytes((0xA5, 0x5A)):
            yield timestamp, pixel[0] | pixel[1] << 8


def producer_writes(path):
    data = path.read_bytes()
    if len(data) % 10:
        raise ValueError("truncated producer write record")
    return [struct.unpack_from("=QH", data, offset) for offset in range(0, len(data), 10)]


def frame_ends(path):
    wire = bytearray()
    ends, times = [], []
    for timestamp, data in wire_records(path):
        wire.extend(data)
        ends.append(len(wire))
        times.append(timestamp)
    return [times[bisect.bisect_left(ends, match.end())]
            for match in re.finditer(rb"\x1b\[\?2026l", wire)]


def latency_summary(writes, marks, ends, started, ended, pool):
    """Producer write completion to the host write that ends the frame carrying that upload.

    Frames may be skipped but never reordered, so each presented fixture matches the next
    producer write of that fixture. A skip of a whole pool cycle is indistinguishable and would
    overstate latency by one cycle, so the longest skip is reported alongside the result.
    """
    latencies, unmatched, longest_skip, cursor = [], 0, 0, 0
    for marked, index in marks:
        position = bisect.bisect_left(ends, marked)
        end = ends[position] if position < len(ends) else None
        match = next((position for position in range(cursor, len(writes))
                      if writes[position][1] == index), None)
        if end is None or match is None or writes[match][0] > end:
            unmatched += 1
            continue
        longest_skip = max(longest_skip, match - cursor)
        cursor = match + 1
        if started <= end <= ended:
            latencies.append((end - writes[match][0]) / 1000)
    return {"boundary": "producer write return -> host write ending the frame with that upload",
            "samples": len(latencies), "unmatched_uploads": unmatched,
            "longest_skipped_producer_frames": longest_skip,
            "ambiguous": longest_skip >= pool - 1,
            "latency_us": percentiles(latencies)}


def echo_summary(probes, echoes, path):
    """Time each probe's tagged echo through the producer to the host write that completes it.

    `send_ns` precedes starting the CLI that injects the key, so the input leg includes its
    process start-up; the output leg starts when the producer has written the echo.
    """
    wire, ends, times = bytearray(), [], []
    for timestamp, data in wire_records(path):
        wire.extend(data)
        ends.append(len(wire))
        times.append(timestamp)
    echoed = {echo["input"]: echo for echo in echoes}
    legs = {"send_to_producer_read_us": [], "producer_echo_to_host_us": [], "send_to_host_us": []}
    missing = 0
    for probe in probes:
        echo = echoed.get(probe["token"])
        found = wire.find(f"echo:{probe['token']};".encode())
        if echo is None or found < 0:
            missing += 1
            continue
        host = times[bisect.bisect_left(ends, found + len(probe["token"]) + 6)]
        legs["send_to_producer_read_us"].append((echo["read_ns"] - probe["send_ns"]) / 1000)
        legs["producer_echo_to_host_us"].append((host - echo["echoed_ns"]) / 1000)
        legs["send_to_host_us"].append((host - probe["send_ns"]) / 1000)
    return {"probes": len(probes), "missing": missing} | {
        name: percentiles(values) for name, values in legs.items()}


def visibility_summary(visibility, writes, marks):
    """Presentation while hidden, and how fresh the first frame after reveal is."""
    events = {event["action"]: event["ns"] for event in visibility}
    hidden, revealed = events.get("hide"), events.get("reveal")
    if hidden is None:
        return None
    end = revealed or (marks[-1][0] if marks else hidden)
    report = {"hidden_seconds": (end - hidden) / 1e9,
              "presented_while_hidden": sum(hidden <= mark < end for mark, _ in marks),
              "producer_writes_while_hidden": sum(hidden <= write < end for write, _ in writes)}
    after = [(mark, index) for mark, index in marks if revealed and mark >= revealed]
    if after:
        mark, index = after[0]
        written = max((write for write, fixture in writes if fixture == index and write <= mark),
                      default=None)
        report["reveal_to_first_presentation_us"] = (mark - revealed) / 1000
        report["first_revealed_frame_age_us"] = None if written is None else (mark - written) / 1000
    return report


def isolated_environment(root):
    # The owned window is always local; an inherited SSH session would select remote transport.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith(("ROZI", "SSH_"))}
    for key, name in [("XDG_RUNTIME_DIR", "run"), ("XDG_CONFIG_HOME", "config"),
                      ("XDG_DATA_HOME", "data"), ("XDG_STATE_HOME", "state"),
                      ("XDG_CACHE_HOME", "cache")]:
        (root / name).mkdir(mode=0o700)
        env[key] = str(root / name)
    return env


def owned_processes(root, client_pid):
    pids = {"client": client_pid, "producer": int((root / "producer.pid").read_text()),
            "ghostty": json.loads((root / "window.json").read_text())["pid"]}
    children = Path(f"/proc/{client_pid}/task/{client_pid}/children").read_text().split()
    for child in children:
        cmdline = Path(f"/proc/{child}/cmdline").read_bytes().split(b"\0")
        if b"--server" in cmdline or b"--fresh-server" in cmdline:
            pids["server"] = int(child)
    return pids


def benchmark(args):
    root = args.output.resolve()
    root.mkdir(mode=0o700, parents=True, exist_ok=False)
    binary = str(args.binary.resolve(strict=True))
    helper = str(args.window_lab.resolve(strict=True))
    producer = Path(__file__).with_name("graphics-stream.py").resolve()
    env = isolated_environment(root)
    wrapper = root / "producer.py"
    producer_args = [sys.executable, str(producer), "--fps", str(args.producer_fps),
                     "--duration", str(args.seconds + 2), "--mode", args.mode,
                     "--width", str(args.image_width), "--height", str(args.image_height),
                     "--ready-file", str(root / "producer-ready.json"),
                     "--write-log", str(root / "producer-writes.bin"),
                     *(["--echo-input"] if args.input_probes else []),
                     "--files", str(root / "frames"), "--report", str(root / "producer.json")]
    wrapper.write_text("import os,pathlib,time\n"
                       f"root=pathlib.Path({str(root)!r})\n"
                       "(root/'producer.pid').write_text(str(os.getpid()))\n"
                       "print('graphics benchmark ready',flush=True)\n"
                       "while not (root/'start').exists(): time.sleep(.01)\n"
                       f"os.execv({sys.executable!r},{producer_args!r})\n")
    config = root / "config" / "rozi"
    config.mkdir()
    (config / "config.toml").write_text(
        f"frame_rate = {args.fps}\nshell = {json.dumps([sys.executable, str(wrapper)])}\n"
        '[session]\nautosave = false\nresurrect = false\n'
        '[animations]\nenabled = false\n[confirm]\nkill_session = false\n'
        '[shell_integration]\nmode = "off"\n')
    lab = [sys.executable, helper, "--root", str(root)]
    session = "graphics-benchmark"
    launched = False
    try:
        launch = [*lab, "launch", "--width", str(args.width), "--height", str(args.height),
                  "--workspace", str(args.workspace), "--font-size", "8", "--cwd", str(root)]
        for key in sorted(os.environ):
            if key.startswith("SSH_"):
                launch += ["--unset", key]
        if args.wiretap:
            launch += ["--env", f"LD_PRELOAD={args.wiretap.resolve(strict=True)}",
                       "--env", f"GRAPHICS_WIRETAP_EXE={binary}",
                       "--env", f"GRAPHICS_WIRETAP_PATH={root / 'host-wire.bin'}",
                       "--env", f"GRAPHICS_WIRETAP_MEDIA_PATH={root / 'host-media.bin'}"]
        run([*launch, "--", binary, "sessions", "new", session, "--cwd", str(root)])
        launched = True
        socket = wait_for(lambda: next((root / "run/rozi").glob("control-*.sock"), None),
                          "UI control socket")
        wait_for(lambda: (root / "producer.pid").exists(), "producer")
        client_pid = int(socket.stem.removeprefix("control-"))

        def metrics():
            return json.loads(run([binary, "--socket", str(socket), "metrics", "--format", "json"], env))["data"]

        before = metrics()
        (root / "start").touch()
        wait_for(lambda: (root / "producer-ready.json").exists(), "immutable fixture pool")
        time.sleep(1)
        start_metrics = metrics()
        pids = owned_processes(root, client_pid)
        initial = {name: process_sample(pid) for name, pid in pids.items()}
        started = time.monotonic()
        started_ns = time.monotonic_ns()
        probes, visibility = [], []
        schedule = [(args.seconds * (number + 1) / (args.input_probes + 1), "probe", number)
                    for number in range(args.input_probes)]
        schedule += [(at, action, None) for at, action in [(args.hide_at, "hide"),
                                                           (args.reveal_at, "reveal")] if at]
        if schedule:
            pane = str(json.loads(run([binary, "--socket", str(socket), "list-panes",
                                       "--format", "json"], env))["data"][0]["id"])
            window_class = json.loads((root / "window.json").read_text())["class"]
        for at, action, number in sorted(schedule):
            time.sleep(max(0, started + at - time.monotonic()))
            if action != "probe":
                # Another Rozi workspace hides the pane inside the owned window only.
                visibility.append({"action": action, "ns": time.monotonic_ns()})
                run([binary, "--socket", str(socket), "switch-workspace",
                     "2" if action == "hide" else "1"], env)
                continue
            # A host key arrives as one keystroke, so its token is one letter.
            token = chr(ord("a") + number % 26) if args.input_via == "host" else f"k{number}"
            probes.append({"token": token, "send_ns": time.monotonic_ns()})
            if args.input_via == "host":
                # Delivered to the owned window by class, without moving desktop focus.
                shortcut = '{mods="",key=%s,window=%s}' % (
                    json.dumps(token), json.dumps(f"class:{window_class}"))
                run(["hyprctl", "eval", f"hl.dispatch(hl.dsp.send_shortcut({shortcut}))"])
            else:
                run([binary, "--socket", str(socket), "send-text", "--target", pane, token],
                    env)
        time.sleep(max(0, started + args.seconds - time.monotonic()))
        ended_ns = time.monotonic_ns()
        elapsed = time.monotonic() - started
        final = {name: process_sample(pid) for name, pid in pids.items()}
        end_metrics = metrics()
        report = {"binary": binary, "binary_sha256": hashlib.sha256(Path(binary).read_bytes()).hexdigest(),
                  "version": run([binary, "--version"], env).strip(), "fps": args.fps,
                  "producer_fps": args.producer_fps, "mode": args.mode, "sample_seconds": elapsed,
                  "sample_started_ns": started_ns, "sample_ended_ns": ended_ns,
                  "processes": {name: cpu_report(initial[name], final[name], elapsed) for name in pids},
                  "before_stream": before, "start_metrics": start_metrics, "end_metrics": end_metrics,
                  "window": json.loads(run([*lab, "status"])),
                  "qualification": "Host frame counters measure successful writes, not monitor scanout."
                  " Interval percentiles include the newest 1024 frames; idle intervals can remain."}
        wait_for(lambda: (root / "producer.json").exists(), "producer completion")
        report["producer"] = json.loads((root / "producer.json").read_text())
        if args.wiretap:
            report["host_wire"] = wire_summary(root / "host-wire.bin", started_ns, ended_ns,
                                               1e6 / args.fps)
            media = root / "host-media.bin"
            marks = list(media_marks(media)) if media.exists() else []
            report["latency"] = latency_summary(
                producer_writes(root / "producer-writes.bin"), marks,
                frame_ends(root / "host-wire.bin"), started_ns, ended_ns,
                report["producer"]["fixture_files"])
            report["latency"]["marked_uploads"] = len(marks)
            if visibility:
                report["visibility"] = visibility_summary(
                    visibility, producer_writes(root / "producer-writes.bin"), marks)
            if probes:
                report["input_echo"] = echo_summary(
                    probes, report["producer"]["echoes"], root / "host-wire.bin")
        (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
        print(json.dumps({"result": str(root / "result.json"), "seconds": elapsed}))
    finally:
        subprocess.run([binary, "sessions", "kill", session], env=env,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=20)
        if launched:
            subprocess.run([*lab, "close"], timeout=20)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--window-lab", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--wiretap", type=Path)
    parser.add_argument("--fps", type=int, choices=[60, 120, 144, 240], default=120)
    parser.add_argument("--producer-fps", type=int, choices=[30, 60, 120, 144, 240], default=240)
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--input-probes", type=int, default=0,
                        help="send this many keys during the sample and time their echoes")
    parser.add_argument("--hide-at", type=float,
                        help="seconds into the sample to switch the Rozi UI to workspace 2")
    parser.add_argument("--reveal-at", type=float,
                        help="seconds into the sample to switch back to workspace 1")
    parser.add_argument("--input-via", choices=["control", "host"], default="host",
                        help="host keys go through Ghostty and the client's input path; control "
                             "keys enter through the session control socket")
    parser.add_argument("--mode", choices=["unchanged", "moving", "whole"], default="moving")
    parser.add_argument("--workspace", type=int, default=2)
    parser.add_argument("--width", type=int, default=980)
    parser.add_argument("--height", type=int, default=720)
    parser.add_argument("--image-width", type=int, default=640)
    parser.add_argument("--image-height", type=int, default=320)
    args = parser.parse_args()
    if args.seconds <= 0:
        parser.error("--seconds must be positive")
    if args.reveal_at and not (args.hide_at and args.hide_at < args.reveal_at < args.seconds):
        parser.error("--reveal-at needs an earlier --hide-at, both within --seconds")
    if args.input_via == "host" and args.input_probes > 26:
        parser.error("host input probes use distinct letters; at most 26")
    benchmark(args)


if __name__ == "__main__":
    main()
