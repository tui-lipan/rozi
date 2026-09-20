<p align="center">
  <img src="assets/logo.png" alt="rozi" width="140">
</p>

<h1 align="center">rozi</h1>

<p align="center">
  A modern terminal workspace for local, remote, and agent-driven development.
</p>

<p align="center">
  <a href="https://rozi.tui-lipan.dev">Website</a>
  ·
  <a href="https://github.com/tui-lipan/rozi/actions/workflows/ci.yml">CI</a>
  ·
  <a href="LICENSE">MPL-2.0</a>
</p>

<p align="center">
  <img src="assets/demo.gif" alt="A rozi session opening, splitting, resizing, and arranging terminal panes" width="860">
</p>

rozi is a terminal multiplexer that tiles terminals like a window manager, keeps named sessions
alive after you detach, and connects to sessions on other machines over SSH. It tracks coding-agent
activity without treating agents differently from shells, editors, monitors, or any other terminal
program. Its CLI, hooks, and extensions automate the same session runtime used by the interactive
client.

| Tiling workspace | Persistent sessions | Remote machines |
| --- | --- | --- |
| Dwindle, master, grid, scrolling, floating, and fullscreen layouts | Live processes and scrollback survive detach | Attach to saved hosts through SSH |
| **Agent activity** | **Shared sessions** | **Automation and extensions** |
| See which coding agents are working or need input | Follow another client or take layout control | Inspect, control, and extend rozi from the CLI |

Tiling behavior and keyboard flow take their cues from the
[Hyprland](https://hypr.land) window manager.

## Install

```bash
curl -fsSL https://rozi.tui-lipan.dev/install | bash
```

```powershell
irm https://rozi.tui-lipan.dev/install.ps1 | iex
```

You can also install with Cargo:

```bash
cargo install rozi --locked
```

Building from source requires Rust 1.90 or newer. See [Installation](docs/installation.md) for
PATH setup, updates, rollback, and source builds.

There are also [nightly builds](docs/installation.md#nightly-builds): a disposable binary of the
newest `master` commit that passed CI, for trying a fix before it is released. They are unsigned,
replaced every night, and no install or update path selects them.

## First five minutes

Run `rozi`. The session picker opens without creating or attaching to a session.

For a shell you can discard, press `Enter` or `Ctrl+T`. For work you want to return to, type a
session name such as `dev` and press `Ctrl+N`.

The default prefix is `Ctrl+A`. Press it, release it, then press the command key:

| Keys | Action |
| --- | --- |
| `Ctrl+A`, `Enter` | Split the focused pane |
| `Ctrl+A`, `h` / `j` / `k` / `l` | Move focus |
| `Ctrl+A`, `p` | Open the command palette |
| `Ctrl+A`, `?` | Show active keybindings |
| `Ctrl+A`, `d` | Detach from a named session |

Attach to the named session again with:

```bash
rozi sessions attach dev
```

## Documentation

- [Overview](docs/overview.md)
- [Getting started](docs/getting-started.md)
- [Feature map](docs/features.md)
- [Core concepts](docs/core-concepts.md)
- [Keybindings](docs/keybindings.md)
- [Configuration](docs/configuration.md)
- [Platform support](docs/platform-support.md)

The [documentation index](docs/index.md) links to every guide.

## Platforms

rozi supports Linux, macOS, and Windows. Windows requires Windows 10 version 1809 or newer.
Some process inspection and shell integration details differ by platform.

It also builds and runs on NetBSD, where it is packaged in pkgsrc. That platform is
community-supported: no prebuilt binaries, and CI builds and tests it without gating a merge.

See [Platform support](docs/platform-support.md).

## Contributing

Contributions require a Developer Certificate of Origin sign-off. See
[CONTRIBUTING.md](CONTRIBUTING.md).

## License

rozi is licensed under the [Mozilla Public License 2.0](LICENSE).
