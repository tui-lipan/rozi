# Record a pane

`rozi record` records a pane's screen over time, change by change, to a file you can replay in a
terminal or export to PNG frames, a GIF, a video, or an asciinema cast. The session server does the
recording, so it runs with no UI attached: start one before you detach, and review what a coding
agent did overnight in the morning.

## Record a pane

Name the session, the pane, and the file:

```sh
rozi --session dev record start pane --target 3 --output agent.rozirec
```

The command returns at once and the recording runs in the session server until you stop it. Find
pane ids with `rozi --session dev list-panes`. Add a label to the current moment, see what is
running, and stop:

```sh
rozi --session dev record mark "tests started"
rozi --session dev record list
rozi --session dev record stop
```

`record stop` answers once the file is complete. With more than one recording running, name one
with `--id`, from `record list`. `record mark` labels every running recording unless given `--id`,
and fails for a recording that is already ending, so a mark it reports was written.

To record only while a command runs in the foreground, use `record pane`. It records until you press
`Ctrl+C`. If the recording ends on its own first, such as when the pane's program exits, it prints
how it ended:

```sh
rozi --session dev record pane --target 3 --output demo.rozirec
```

Recording always starts from one of these commands. Nothing records in the background on its own.

## What a recording holds

A pane recording holds the pane's own terminal screen, the one `capture-pane --session` returns:
its text, colors, cursor, and the images programs displayed. It does not show anything rozi draws
around the pane, such as borders, titles, the bar, or overlays, and it looks the same whether or not
a UI is attached.

A frame is written only when the screen changes, so a pane that sits still costs nothing, and each
change is written at the moment it happened. `--max-fps` caps how often: changes that arrive faster,
such as an animation or a program flooding output, are merged into the latest screen for each
interval.

A recording also notes, with their times:

- your marks;
- title changes;
- commands starting and finishing, from [shell integration](terminal.md#working-directories-and-shell-metadata);
- the detected agent and its state, from [agent detection](agents.md);
- statuses set with `rozi status`;
- the pane's program exiting, with its status.

A recording contains everything the pane shows, including any secrets, tokens, or passwords that
appear on screen. Treat the file like the terminal it came from. rozi creates it readable and
writable only by you, and never replaces an existing file unless you pass `--force`.

## See that a pane is recording

While a pane is recording, every attached UI marks it with a dot and `rec` in the theme's error
color, at the start of its title so a long title never hides it. The dot blinks slowly; with
`[animations] enabled` or `focus_chrome` off it holds steady. With `[pane] show_titles = false`,
the dot sits in the top-left corner of the pane's border instead. A workspace tab carries the dot
while its workspace holds a recorded pane you cannot see: one on another workspace, or one with
neither a title nor a border to show it. The indicator is rozi's own chrome, so it never appears in
the recording.

`list-panes` reports `"recording": true` for the pane, `record list` shows the file and its
progress, and `metrics` counts recordings in its `recordings` section.

## Where the file goes

The session server writes the file, so the path is on the session's host. A relative `--output` is
resolved against the directory you run `rozi` in. With `--remote HOST --session NAME`, the file is
written on that host and a relative path is resolved against your login directory there, usually
your home directory. The directory must already exist.

## Limits and how a recording ends

| Option | Default | Meaning |
| --- | --- | --- |
| `--max-fps N` | `30` | The most frames a second, from 1 to 120. |
| `--duration DUR` | `24h` | Stop after this long, such as `90s`, `8h`, or `1h30m`; at most `7d`. |
| `--max-bytes SIZE` | `1GiB` | Stop before the file grows past this size, such as `512MiB`; at least `64KiB`. |
| `--force` | off | Replace an existing file at `--output`. |

A recording ends when you stop it, when it reaches its duration or size, when the pane's program
exits or the pane closes, or when the session is killed or its server stops. Its last event says
which. A foreground `record pane` also stops when the command exits, including when you press
`Ctrl+C` or its terminal closes.

The file is valid whichever way it ends. rozi writes it as it goes and flushes it at least once a
second, so a file cut short by a crash or a full disk still plays and exports up to its last
complete event.

If the disk cannot keep up, rozi keeps the latest screen rather than slowing the pane down, and the
end of the file counts the changes it skipped as `dropped`. The session never waits on a recording.

## Play a recording

`record play` replays a recording in your terminal at its own pace:

```sh
rozi record play agent.rozirec
rozi record play agent.rozirec --speed 4
rozi record play agent.rozirec --from "tests started"
rozi record play agent.rozirec --from 1h30m
```

`--from` starts at a mark, by its label, or at a time into the recording, on the screen as it was
at that moment. Playback lasts until the recording ended, holding the last screen for as long as it
stood still. It uses your terminal's colors, so make the terminal at least as large as the recorded
pane. Press `Ctrl+C` to stop.

## Export frames, a GIF, or a video

`record export --to png-frames` writes each frame as a PNG in the recording's colors, and a
`frames.ffconcat` listing that gives each frame its real duration:

```sh
rozi record export agent.rozirec --to png-frames frames --scale 2
```

`--scale` enlarges the frames, from 1 to 3. Hand the listing to [ffmpeg](https://ffmpeg.org) to make
a GIF or a video; rozi does not encode video itself:

```sh
ffmpeg -f concat -safe 0 -i frames/frames.ffconcat \
  -vf "split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=none" demo.gif
ffmpeg -f concat -safe 0 -i frames/frames.ffconcat \
  -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" -pix_fmt yuv420p -fps_mode vfr demo.mp4
```

`--to cast` writes an [asciinema](https://asciinema.org) cast, version 2, which plays in a browser
and keeps the text selectable:

```sh
rozi record export agent.rozirec --to cast agent.cast
```

A cast is text, so images appear as the half-block approximations their cells hold. Marks become
cast markers, which players list as chapters to jump to, and when the pane changes size the cast's
terminal changes size with it. Neither export replaces existing files unless you pass `--force`.

Export and play read the file on the machine you run them on and need no session.

## Cost

Recording runs beside the session without slowing it. The session server only notices a change
and takes the screen; a separate thread in the server turns it into the file. Measured with a
release build on Linux, a 120×32 pane, and the defaults:

| Pane running | Server CPU without | Server CPU recording | File growth | Frames a second |
| --- | --- | --- | --- | --- |
| An agent-like spinner, updating 10 times a second | 1.1% | 2.5% | 0.2 MiB a minute | 10 |
| `btop -u 100` | 1.6% | 5.6% | 5.7 MiB a minute | 11 |
| `seq` flooding output | 175% | 177% | 3.9 MiB a minute | 30 |

No change was dropped in any of these. A dashboard that redraws most of its screen costs the most
disk; set `--max-bytes` or `--max-fps` to bound a long recording of one.

## The file format

A recording is line-delimited JSON in the `rozi-recording` format: a header line, then one event per
line, with frames in the [`rozi-spans`](control-protocol.md#spans-frames) format. It is documented
in [Recording format](control-protocol.md#recording-format) and described by the
[JSON schema](control.md#json-schema), so other tools can read it.
