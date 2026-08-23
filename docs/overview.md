# Overview

rozi is a modern tiling terminal multiplexer. Its tiling and animations take their cues from the
Hyprland window manager. It arranges live terminal panes across workspaces and gives you keyboard
and mouse controls for focus, layout, floating, fullscreen, and resizing.

Every pane belongs to a session server. A client can display and control a session, then leave
without stopping a named session. A bare `rozi` launch starts at the session picker and does not
create a shell until you choose one.

## New to rozi

Read [Getting started](getting-started.md) for a guided first session. Use
[Installation](installation.md) for install, update, and rollback commands. The
[Platform support](platform-support.md) page covers operating system requirements and differences.

The [Feature map](features.md) groups capabilities by what you may want to do. Read
[Core concepts](core-concepts.md) for the terms used throughout the documentation.

## Daily use

| Guide | Covers |
| --- | --- |
| [Keybindings](keybindings.md) | Prefix commands, modifier shortcuts, modes, and mouse controls |
| [Layouts and panes](layouts-and-panes.md) | Tiling layouts, focus, movement, floating panes, and fullscreen |
| [Terminal features](terminal.md) | Scrollback, search, copy mode, clipboard, links, images, and shell integration |
| [Sessions](sessions.md) | Named and temporary sessions, detach, reattach, recovery, and shared control |
| [Shared sessions](shared-sessions.md) | Following, layout control, read-only clients, and client removal |
| [Remote sessions](remote.md) | Attaching to a session on another machine over SSH |
| [Sidebar](sidebar.md) | Pane, session, file, Git, and agent views |

## Set up your environment

| Guide | Covers |
| --- | --- |
| [Configuration](configuration.md) | The `config.toml` reference and live reload |
| [Themes](themes.md) | Built-in, system, and custom themes |
| [Profiles](profiles.md) | Reusable launch recipes, captured layouts, and profile files |
| [Vim and Neovim navigator](../integrations/vim-rozi-navigator/) | One set of navigation keys for editor splits and rozi panes |

## Coding agents

| Guide | Covers |
| --- | --- |
| [Agent activity](sidebar.md#activity) | How rozi shows working, blocked, and finished agents |
| [Agent definitions](agents.md) | Adding or overriding coding-agent detection rules |
| [Agent skill](agent-skill.md) | Installing the rozi control skill for coding agents |

## Automation and extensions

| Guide | Covers |
| --- | --- |
| [Scripting](scripting.md) | Common automation tasks using the `rozi` command |
| [Control CLI](control.md) | Inspecting and controlling a running client from scripts |
| [Control protocol](control-protocol.md) | The raw transport for clients that cannot invoke the CLI |
| [Hooks](hooks.md) | Running commands in response to rozi events |
| [Extensions](extensions.md) | Installing, authoring, and managing extensions |
| [Extension testing](extension-testing.md) | Testing extension commands and lifecycle behavior |
| [Automation recipes](recipes.md) | Complete scripting and extension examples |

## Development

| Guide | Covers |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | Setup, checks, pull requests, and DCO sign-off |
| [Release process](release-process.md) | Publishing and checking a release |
| [Benchmarks and profiling](benchmarks.md) | Running the benchmark and profiling tools |
| [Performance audit archive](performance/README.md) | Recorded measurements and audit history |
| [Performance audit playbook](performance/audit-playbook.md) | Reproducing a full performance audit |

Read the [security policy](../SECURITY.md) before reporting a vulnerability or testing a suspected
security issue.
