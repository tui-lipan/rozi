# Overview

rozi is a tiling terminal multiplexer: it arranges terminal panes the way a tiling window manager
arranges windows. It keeps named sessions running after you leave, reaches other machines over SSH,
and shows which coding agents need you. You can change any of it live, from inside rozi.

Shells, editors, system monitors, test servers, and coding agents are all ordinary panes. The
sidebar can additionally show which coding agents are working, finished, or waiting for your input,
on this machine and on connected hosts.

Tiling behavior, animations, and keyboard flow take their cues from the
[Hyprland](https://hypr.land) window manager.

<CaptureGallery title="~/src/rozi — dev">
<img src="./assets/captures/workspace.webp" alt="A rozi workspace with Neovim, a Git log, lazygit, and btop tiled in one session" data-caption="One workspace in a named session: an editor, a Git log, lazygit, and btop, tiled by the dwindle layout.">
</CaptureGallery>

## New to rozi

| Guide | Covers |
| --- | --- |
| [Getting started](getting-started.md) | A guided first session: create, split, detach, and reattach |
| [Installation](installation.md) | Installing, updating, and rolling back |
| [Core concepts](core-concepts.md) | Sessions, panes, workspaces, layouts, and profiles |
| [Feature map](features.md) | What rozi can do, grouped by task |
| [Platform support](platform-support.md) | Supported operating systems and their differences |
| [Troubleshooting](troubleshooting.md) | Common problems by symptom, with causes and fixes |

## Daily use

| Guide | Covers |
| --- | --- |
| [Keybindings](keybindings.md) | Default keys, the prefix, modes, mouse controls, and rebinding |
| [Panes and layouts](layouts-and-panes.md) | Tiling layouts, focus, movement, floating panes, and fullscreen |
| [Terminal features](terminal.md) | Scrollback, search, copy mode, clipboard, links, images, and shell integration |
| [Sessions](sessions.md) | Named and temporary sessions, the session picker, and resurrection |
| [Worktrees](worktrees.md) | Opening each Git worktree in its own session |
| [Sidebar](sidebar.md) | Pane, session, file, Git, worktree, and agent views |

## Remote and collaboration

| Guide | Covers |
| --- | --- |
| [Remote sessions](remote.md) | Working in sessions on another machine over SSH |
| [Shared sessions](shared-sessions.md) | Several clients in one session: following, layout control, and read-only access |

## Customize

| Guide | Covers |
| --- | --- |
| [Customize rozi](customize.md) | A guided tour of themes, layouts, pane styles, bars, animations, and keys |
| [Configuration](configuration.md) | The `config.toml` reference and live reload |
| [Themes](themes.md) | Built-in, system, and custom themes |
| [Profiles](profiles.md) | Reusable launch recipes and saved layouts |
| [Vim and Neovim navigator](https://github.com/tui-lipan/vim-rozi-navigator) | One set of navigation keys for editor splits and rozi panes |

## Coding agents

| Guide | Covers |
| --- | --- |
| [Agent activity](sidebar.md#activity) | How rozi shows working, blocked, and finished agents |
| [Agent definitions](agents.md) | Adding or overriding coding-agent detection rules |
| [Agent skill](agent-skill.md) | Teaching a coding agent to control rozi |

## Automate and extend

| Guide | Covers |
| --- | --- |
| [Scripting quick start](scripting.md) | Common automation tasks with the `rozi` command |
| [Control CLI](control.md) | The full reference for inspecting and controlling rozi from scripts |
| [Control protocol](control-protocol.md) | The raw transport, for clients that cannot run the CLI |
| [Record a pane or the UI](recording.md) | Record a pane or the whole UI over time, then replay it or export frames, a GIF, or a cast |
| [Hooks](hooks.md) | Running commands when rozi events happen |
| [Extensions](extensions.md) | Installing, writing, and managing extensions |
| [Automation recipes](recipes.md) | Complete scripting and extension examples |

## Development

| Guide | Covers |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | Setup, checks, pull requests, and DCO sign-off |
| [Extension testing](extension-testing.md) | Testing extension commands and lifecycle behavior |
| [Release process](release-process.md) | Publishing and checking a release |
| [Benchmarks and profiling](benchmarks.md) | Running the benchmark and profiling tools |
| [Performance audit archive](performance/README.md) | Recorded measurements and audit history |
| [Performance audit playbook](performance/audit-playbook.md) | Reproducing a full performance audit |

To report a vulnerability, read the [security policy](../SECURITY.md) first.
