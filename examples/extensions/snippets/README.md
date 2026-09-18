# Snippets extension

A picker of commands you save yourself. Selecting a row pastes it into the focused pane. `Ctrl-N`
opens a stacked prompt for a new command and keeps the picker open so you can pick it next. An empty
list shows `No snippets yet` rather than a fake row.

This extension uses the Python standard library and the public `rozi` CLI. It does not import Rozi
source.

## What it gives you

- `snippets.pick` (`Ctrl+A x s`) — saved commands, grouped if some also come from config
- `Enter` pastes the highlighted command into the focused pane, without pressing Enter
- `Ctrl-N` prompts for a command, stores it, and refreshes the open picker
- `Ctrl-D` twice deletes a saved row

The chord is a suggestion. If Rozi already bound `s` inside `<prefix> x`, bind `snippets.pick`
yourself in `[keys]`.

## Requirements

- Rozi with extension API 1 and the streaming `rozi pick --json` protocol
- Python 3.10 or newer available as `python`

## Install

```sh
rozi extensions check ./snippets
rozi extensions install --link ./snippets
rozi run-action reload-extensions
```

## Settings

```toml
[extensions.snippets]
submit = false
commands = ["git status", "git diff"]
```

`submit = true` also sends Enter after the paste. `commands` are listed under **From config** and
cannot be deleted from the picker; change them in `config.toml`.

## State

Saved rows live in `$XDG_STATE_HOME/rozi-snippets/commands/<id>.json` (or
`~/.local/state/rozi-snippets/commands/`), never inside the installed extension directory. Each
saved command is its own file, so two Rozi clients can add or delete at the same time without
overwriting each other.

## Tests

```sh
cd snippets && python -m unittest discover -s tests
```

## Manual check

```sh
rozi run-action snippets.pick
```

1. An empty list shows `No snippets yet`. Press `Ctrl-N`, enter `echo hello`, and confirm the row
   appears under **Saved**.
2. Select it with `Enter` and verify the focused pane receives `echo hello` without a newline.
3. Reopen the picker, highlight the row, press `Ctrl-D` twice, and verify it disappears.
4. Press `Esc` and verify the picker closes without an error toast.
