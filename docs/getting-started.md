# Getting started

This guide starts a named session, opens a second pane, detaches, and attaches again.

## 1. Install rozi

On Linux or macOS:

```bash
curl -fsSL https://rozi.tui-lipan.dev/install | bash
```

On Windows PowerShell:

```powershell
irm https://rozi.tui-lipan.dev/install.ps1 | iex
```

You can also use `cargo install rozi` with Rust 1.90 or newer. See
[Installation](installation.md) for PATH setup, source builds, updates, and rollback.

## 2. Open the session picker

Run:

```bash
rozi
```

The session picker opens. At this point rozi has not created or attached to a session.

Press `Enter` to start a temporary shell. `Ctrl+T` does the same thing. Temporary sessions are
useful for short work, but they are not durable.

## 3. Create a named session

For work you want to return to, type `dev` in the picker and press `Ctrl+N`. The new named session
opens with a shell.

Named sessions keep running after the last client detaches. They stop only when you kill them.

## 4. Work with panes

The default prefix is `Ctrl+A`. Press the prefix, release it, then press a command key.

Open another pane:

```text
Ctrl+A, then Enter
```

Move focus with:

```text
Ctrl+A, then h, j, k, or l
```

The directions follow Vim: left, down, up, and right. See
[Layouts and panes](layouts-and-panes.md) for layouts, resizing, floating panes, and fullscreen.

## 5. Detach

Leave the named session running:

```text
Ctrl+A, then d
```

The client exits. The shells in `dev` continue running.

## 6. Attach again

```bash
rozi attach dev
```

You return to the same live panes and scrollback.

## 7. Find commands and keys

Open the command palette with `Ctrl+A`, then `p`. Open the keybinding help with `Ctrl+A`, then `?`.
Both show your active configuration, including any rebinding.

## Next

- [Core concepts](core-concepts.md) explains panes, workspaces, sessions, and profiles.
- [Feature map](features.md) points to the guides for each part of rozi.
- [Keybindings](keybindings.md) lists the default controls.
- [Configuration](configuration.md) covers `config.toml` and live reload.
- [Platform support](platform-support.md) lists operating system requirements and differences.
