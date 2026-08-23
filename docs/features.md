# Feature map

rozi combines terminal panes with the layout controls of a tiling window manager. This page points
to the guide for each kind of work.

## Arrange terminal work

- Split the focused pane and let the current layout place the new pane.
- Choose between dwindle, master, grid, columns, rows, scrollable, and monocle layouts.
- Move, swap, resize, float, or fullscreen panes.
- Organize panes across nine workspaces.
- Use the mouse for focus, split resizing, and floating pane movement.

See [Layouts and panes](layouts-and-panes.md) and [Keybindings](keybindings.md).

## Use a full terminal

- Run interactive programs in PTY-backed panes.
- Search scrollback and copy with keyboard motions.
- Select text, use the clipboard, open links, and show terminal images.
- Forward mouse input to programs that request it.
- Use shell integration for working directories, prompt boundaries, and the last command's output.

See [Terminal features](terminal.md).

## Leave work running

- Create named sessions that keep their live panes after clients detach.
- Attach several clients to one named session and hand layout control between them.
- Attach to sessions on another machine over SSH.
- Use temporary sessions for work that does not need a durable name.
- Save reusable launch setups as profiles.

See [Sessions](sessions.md), [Remote sessions](remote.md), and [Profiles](profiles.md).

## Find and monitor work

- Open commands from a searchable palette and inspect active bindings in the help overlay.
- Use the sidebar to browse panes, sessions, files, Git changes, and coding-agent activity.
- Mark coding-agent panes as working, blocked, or finished.
- Jump to panes that need input and show alerts in pane borders or workspace tabs.

See [Sidebar](sidebar.md#activity), [agent definitions](configuration.md#agents), and
[Agent skill](agent-skill.md).

## Change the interface

- Rebind built-in commands or add commands that open a pane and send text.
- Change layouts, borders, gaps, titlebars, animations, and the workbar.
- Choose a built-in or system theme, or add a custom theme file.
- Reload configuration and themes when their files change.

See [Configuration](configuration.md), [Keybindings](keybindings.md), and [Themes](themes.md).

## Automate rozi

- Inspect panes and run actions from scripts.
- Send keys or text, open panes, capture terminal content, and switch workspaces.
- Run hooks when pane, focus, workspace, session, or profile events occur.
- Build out-of-process extensions with commands, services, pickers, activity rows, and notifications.

See [Scripting](scripting.md), [Control CLI](control.md), [Hooks](hooks.md), and
[Automation recipes](recipes.md).

## Use it on your operating system

rozi supports Linux, macOS, and Windows. Shell integration, process inspection, paths, and PTY
support differ where the operating systems require it.

See [Platform support](platform-support.md) and [Installation](installation.md).
