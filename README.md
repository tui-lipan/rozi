<p align="center">
  <img src="assets/logo.png" alt="rozi" width="140">
</p>

<h1 align="center">rozi</h1>

<p align="center">
  A tiling terminal multiplexer. Your terminals, tiled like a window manager.
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

rozi arranges panes as you open them, keeps named sessions running after you leave, reaches other
machines over SSH, and shows which coding agents need you. You can change any of it live, from
inside rozi: themes, pane frames, layouts, and keys preview as you browse and save to one config
file. Everything you can do interactively, you can also script from the CLI, hooks, and extensions.

| Tiling workspace | Persistent sessions | Remote machines |
| --- | --- | --- |
| Seven tiling layouts, plus floating and fullscreen panes | Live processes and scrollback survive detach | Attach to saved hosts through SSH |
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

Run `rozi`. The session picker opens; nothing starts until you choose.

For work you want to return to, type a session name such as `dev` and press `Ctrl+N`. For a
throwaway shell, press `Enter` while the list is empty, or `Ctrl+T` once it shows sessions.

Most commands start with the prefix, `Ctrl+A` by default. Press it, release it, then press the
command key:

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

The full documentation is at [rozi.tui-lipan.dev](https://rozi.tui-lipan.dev). Good places to
start:

- [Getting started](docs/getting-started.md) — a guided first session
- [Core concepts](docs/core-concepts.md) — sessions, panes, workspaces, and profiles
- [Feature map](docs/features.md) — what rozi can do, grouped by task
- [Keybindings](docs/keybindings.md) — the default keys
- [Configuration](docs/configuration.md) — the `config.toml` reference

The [documentation overview](docs/overview.md) lists every guide.

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
