# Feature map

This page lists what rozi can do, grouped by the kind of work, and points to the guide for each.

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
- Save a PNG screenshot of the focused pane or the whole window from the command palette.

See [Terminal features](terminal.md).

## Leave work running

- Create named sessions that keep their live panes after clients detach.
- Attach several clients to one named session and hand layout control between them.
- Attach to sessions on another machine over SSH.
- Use temporary sessions for work that does not need a durable name.
- Save reusable launch setups as profiles.
- Create Git worktrees and open each in its own session, from the sidebar's Worktrees tab, a picker,
  or `rozi worktrees`.

See [Sessions](sessions.md), [Remote sessions](remote.md), [Profiles](profiles.md), and
[Worktrees](worktrees.md).

## Find and monitor work

- Open commands from a searchable palette and inspect active bindings in the help overlay.
- Use the sidebar to browse panes, sessions, files, Git changes, and coding-agent activity.
- See at a glance whether each coding agent is working, blocked on your input, or finished.
- Jump to panes that need input and show alerts in pane borders or workspace tabs.
- See which agents want attention on a connected remote host, without attaching to a session there.
- List every agent on every connected machine in one view, and go to any of them with one key.

See [Sidebar](sidebar.md#activity), [the Agents view](sessions.md#go-to-an-agent),
[agent definitions](configuration.md#agents),
[Agents on a machine you are not in](remote.md#agents-on-a-machine-you-are-not-in), and
[Agent skill](agent-skill.md).

## Change the interface

- Rebind built-in commands or add commands that open a pane and send text.
- Change layouts, borders, gaps, titlebars, animations, and the workbar.
- Choose a built-in or system theme, or add a custom theme file.
- Reload configuration and themes when their files change.

See [Configuration](configuration.md), [Keybindings](keybindings.md), and [Themes](themes.md).

## Automate rozi

- Inspect panes and run actions from scripts.
- Read each workspace's layout and every pane's position, and subscribe to layout changes.
- Float, place, fullscreen, re-tile, move, swap, resize, or close panes by id.
- Send keys or text, open panes, and switch workspaces.
- Capture a pane as plain text, ANSI-colored text, a PNG image, or JSON runs of styled text with
  the cursor and images, or capture the whole UI as drawn.
- Send input and wait for the pane to answer, or capture once a pane shows some text or stops
  changing, instead of sleeping between steps.
- Drive a detached session with no client attached: list, capture, type, and open panes from a
  script or an SSH login that never starts a terminal.
- Record a pane's screen change by change, from the command palette or a script and even with no UI
  attached, then replay it in a terminal or export PNG frames for a GIF or video, or an asciinema
  cast. Record the whole UI as you see it, chrome included, for a demo or a bug report. See
  [Record a pane or the UI](recording.md).
- Run hooks when pane, focus, workspace, session, or profile events occur.
- Build extensions with static navigation targets plus out-of-process commands, services, tabbed
  pickers, activity rows, and notifications.
- Discover public `rozi-extension` repositories in-app and install their exact indexed commits after
  reviewing source, compatibility, and contribution counts.

See [Scripting](scripting.md), [Control CLI](control.md), [Hooks](hooks.md), and
[Automation recipes](recipes.md).

## Use it on your operating system

rozi supports Linux, macOS, and Windows. Shell integration, process inspection, paths, and PTY
support differ where the operating systems require it. NetBSD builds and runs from pkgsrc as a
community-supported platform.

See [Platform support](platform-support.md) and [Installation](installation.md).
