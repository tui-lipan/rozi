# rozi documentation

rozi is a tiling terminal multiplexer. It arranges panes as you open them, keeps named sessions
running after you leave, reaches other machines over SSH, and shows which coding agents need you.

These pages are also published at [rozi.tui-lipan.dev](https://rozi.tui-lipan.dev). New to rozi?
Start with [Getting started](getting-started.md).

## New to rozi

| Guide | Covers |
| --- | --- |
| [Overview](overview.md) | What rozi does, with a link to the guide for each feature |
| [Getting started](getting-started.md) | A guided first session: create, split, detach, and reattach |
| [Installation](installation.md) | Installing, updating, and rolling back |
| [Core concepts](core-concepts.md) | Sessions, panes, workspaces, layouts, and profiles |
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
| [Extensions](extensions.md) | Finding, installing, and managing extensions |
| [Extension gallery](extension-gallery.md) | Public extensions, with screenshots and install commands |
| [Write an extension](writing-extensions.md) | Manifests, commands, services, sidebar tabs, and the runtime environment |
| [Extension testing](extension-testing.md) | Testing an extension in isolation |
| [Automation recipes](recipes.md) | Complete scripting and extension examples |

## Development

| Guide | Covers |
| --- | --- |
| [Contributing](../CONTRIBUTING.md) | Setup, checks, pull requests, and DCO sign-off |
| [Developer guides](../dev/README.md) | Benchmarks, performance audits, and the release process |

To report a vulnerability, read the [security policy](../SECURITY.md) first.
