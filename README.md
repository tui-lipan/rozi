<!-- Drop the app icon at assets/logo.png (square, ~512x512 works well). -->
<p align="center">
  <img src="assets/logo.png" alt="hyprmux" width="140">
</p>

<h1 align="center">hyprmux</h1>

<p align="center">
  <b>A tiling terminal multiplexer that feels like a modern window manager.</b><br>
  Split your terminal into panes, arrange them automatically, and pick up where you left off.
</p>

<p align="center">
  <a href="https://github.com/Razuer/hyprmux/actions/workflows/ci.yml"><img src="https://github.com/Razuer/hyprmux/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <img src="https://img.shields.io/badge/platforms-Linux%20%C2%B7%20macOS%20%C2%B7%20Windows-blue" alt="Platforms">
  <img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-green" alt="License">
</p>

<!--
  Drop a short screen recording at assets/demo.gif (10-20s: splitting panes, switching
  workspaces, a theme change), then uncomment the block below.

<p align="center">
  <img src="assets/demo.gif" alt="hyprmux in action" width="860">
</p>
-->

---

## What it is

hyprmux turns one terminal window into many. Open a new pane and it takes its place next to the
others automatically — no dragging, no manual sizing. Panes can float on top, go fullscreen, and
spread across nine workspaces. Close the window and everything keeps running, ready to be picked
up again from anywhere.

It runs natively on Linux, macOS, and Windows, borrows its layout and keyboard flow from the
[Hyprland](https://hypr.land) window manager, and gets its terminal from
[`tui-lipan`](https://crates.io/crates/tui-lipan).

## Install

Grab the repository, then run the bootstrap script for your system. It fetches the release built
for your machine, checks it, and installs it — no shell files are touched.

```bash
./install.sh      # Linux and macOS
```

```powershell
.\install.ps1     # Windows
```

Later on, `hyprmux update` moves you to a newer version and `hyprmux update --rollback` puts the
previous one back. See [Installation](docs/installation.md).

Building from source needs Rust 1.88 or newer, plus a checkout of
[`tui-lipan`](https://crates.io/crates/tui-lipan) next to this one — see
[Getting started](docs/getting-started.md).

```bash
cargo run
```

## First five minutes

Start it, and you get a single shell. Everything else happens through the **prefix key** — hold
`Ctrl-a`, let go, then press one key:

| | |
| --- | --- |
| `Ctrl-a` `Enter` | Open another pane |
| `Ctrl-a` `h` `j` `k` `l` | Move focus left / down / up / right |
| `Ctrl-a` `f` | Make the focused pane fullscreen |
| `Ctrl-a` `t` | Let the pane float on top |
| `Ctrl-a` `1`…`9` | Jump to a workspace |
| `Ctrl-a` `p` | Search every command by name |
| `Ctrl-a` `?` | Show all keys |
| `Ctrl-a` `d` | Leave (a named session keeps running without you) |

Prefer chords? `Alt+<key>` does the same thing without the prefix. Both are fully rebindable.

## What you get

**Panes that arrange themselves.** New panes split the pane you are looking at, along whichever
side has more room. Seven arrangements are a keypress away — the default *dwindle*, a
master-and-stack, a grid, even columns, even rows, a horizontally scrolling strip, and
one-at-a-time monocle. Any pane can float, go fullscreen, or be dragged and resized with the
mouse, and panes glide into place rather than jumping — turn that off if you'd rather they didn't.

**Nothing is lost when you close the window.** Your panes live on in the background, so leaving
does not kill your work. `hyprmux attach dev` brings it all back — same layout, same running
programs, same scrollback. Several windows can attach to one session at once, and
`--remote myserver` reaches a session on another machine over SSH.

**Change anything without restarting.** Save `hyprmux.toml` and it applies immediately — keys,
colors, status bar, everything — with your panes untouched. It works in the other direction too:
switch a theme or flip a setting from the command palette and hyprmux writes the change back into
your config file, so what you tried out is what you keep. Custom theme files reload the instant
you save them, so you can tune a color and watch it land.

**A real terminal, not an approximation.** Mouse support, text selection, images, scrollback with
search, copy mode with vi-style motions, clipboard paste, and true color. 29 built-in themes ship
with it, plus a `system` theme that follows your desktop and drop-in theme files of your own — and
the colors inside your panes follow whichever one is active.

**The same everywhere.** Linux, macOS, and Windows, natively — same keys, same config file, same
behavior. Nothing to emulate, nothing to install underneath it.

## Also in the box

| | |
| --- | --- |
| Command palette | Fuzzy-search every command by name; a help overlay lists all keys |
| Side panel | A dock for your files, git changes, panes, sessions, running coding agents — or tabs you define |
| Saved layouts | Capture a working setup and relaunch it with the same panes running the same commands |
| Scratchpad | A pane that drops down over your work and hides again on the same key |
| Copy mode & search | Walk the scrollback with vi motions, search it, copy without the mouse |
| Synchronized typing | Send what you type to every pane in the workspace at once |
| Shared control | When several people attach, one drives the layout and can hand control over |
| Scripting | `hyprmux focus`, `send-text`, `new-pane`, `capture-pane` work from any script |
| Hooks | Run your own commands when something happens |
| Your own shortcuts | Bind a key to open a pane or send text, not just to rebind what exists |
| Status bar | Built-in readouts, your own text, or a command that refreshes on a timer |
| Placement rules | Send panes running a given command to a chosen spot automatically |
| Vim/Neovim navigator | One set of keys for editor splits and hyprmux panes |

## Configure it

Everything lives in one file — `~/.config/hyprmux/hyprmux.toml` on Linux and macOS:

```toml
[theme]
name = "catppuccin-mocha"

[input]
prefix = "ctrl-a"           # the key that starts a command
modifier = "alt"            # or "super"

[layout]
default = "dwindle"         # how new workspaces arrange panes

[pane]
border_style = "rounded"
```

You never have to leave the app to edit it: *Open config file* in the command palette opens it in
your editor, in a pane, and the moment you save, hyprmux picks the changes up. A file that won't
parse falls back to the defaults and tells you so — fix it, save again, and you are back.

The [configuration reference](docs/configuration.md) covers every option,
[`examples/hyprmux.toml`](examples/hyprmux.toml) is the same thing as a copyable file with every
setting commented out at its default, and [`examples/`](examples/) has ready-made snippets.

## Documentation

| | |
| --- | --- |
| [Feature overview](docs/features.md) | Everything hyprmux does, on one page |
| [Getting started](docs/getting-started.md) | Requirements, building, running, quitting |
| [Installation](docs/installation.md) | Installing, updating, rolling back |
| [Keybindings](docs/keybindings.md) | The full key reference |
| [Configuration](docs/configuration.md) | Every setting in `hyprmux.toml` |
| [Layouts & panes](docs/layouts-and-panes.md) | Tiling, floating, fullscreen, resizing |
| [Sessions](docs/sessions.md) | Detaching, reattaching, and sharing sessions |
| [Remote sessions](docs/remote.md) | Working on another machine over SSH |
| [Terminal features](docs/terminal.md) | Mouse, selection, clipboard, scrollback |
| [Themes](docs/themes.md) | Presets, custom themes, hot reload |
| [Sidebar](docs/sidebar.md) | The dockable side panel and its tabs |
| [Profiles](docs/profiles.md) | Saving and relaunching layouts |
| [Project profiles](docs/project-profiles.md) | Profile files and pane identity |
| [Control socket](docs/control.md) | Driving hyprmux from scripts |
| [Hooks](docs/hooks.md) | Running commands when events happen |
| [Vim/Neovim navigator](integrations/vim-rozi-navigator/) | One set of keys for editor splits and panes |
| [Benchmarks](docs/benchmarks.md) | Performance suites and profiling |

Working on hyprmux itself? [AGENTS.md](AGENTS.md) has the architecture notes.

## Platforms

Windows needs version 1809 or newer, and a couple of conveniences that read other programs'
details are Unix-only. Everything else is the same on all three — see the
[platform support matrix](docs/getting-started.md#platform-support).

## Sponsor

If hyprmux is useful to you, consider [sponsoring its development](https://github.com/sponsors/Razuer) ♥

## License

MIT OR Apache-2.0.
