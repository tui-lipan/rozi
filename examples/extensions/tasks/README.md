# Tasks extension

Finds the project tasks around the focused pane and runs one in a new pane. Sources are `just`
recipes, `make` targets, and `package.json` scripts, grouped by where they came from.

This extension was written against the public extension surface only — `docs/`,
`schemas/extension.schema.json`, `rozi extensions check`, and `rozi --help` — without reading Rozi's
source. It uses the Python standard library, the `rozi` CLI, and nothing else.

## What it gives you

- `tasks.run` (`Ctrl+A x t`) — a picker of every task in the project, grouped by source, with the
  last task you ran marked active. `Ctrl+R` rescans without closing.
- `tasks.repeat` (`Ctrl+A x .`) — run the last task again, in the project it came from.
- A **Tasks** sidebar tab listing the runnable command for each task under section headers,
  refreshed every 30 seconds. Clicking one types it into the focused pane without a newline, so you
  read it before pressing Enter. A command tab can only `send` the row it was clicked on, so the
  rows carry `make build` rather than `build`.

Both chords are suggestions. Rozi drops one silently if you have already bound it; see
`rozi extensions list --verbose` and bind them yourself in `[keys]` if so.

## Requirements

- Rozi with extension API 1
- Python 3.10 or newer available as `python`
- Whichever runners you actually use (`just`, `make`, `npm`/`pnpm`/`yarn`)

## Install

```sh
data_root="${XDG_DATA_HOME:-$HOME/.local/share}/rozi"
install -d -m 700 "$data_root" "$data_root/extensions"
cp -R ./tasks "$data_root/extensions/tasks"
rozi extensions check "$data_root/extensions/tasks"
rozi run-action reload-extensions
```

Extensions go under the data directory, not beside `config.toml`. The `install -d -m 700` matters:
Rozi's data directory is also its managed-installation root and must stay owner-only.

## Settings

```toml
[extensions.tasks]
sources = ["just", "make"]   # which sources to scan, in the order their groups appear
pane = "background"          # "focus" opens the task pane focused, "background" leaves focus alone
keep_open = true             # hold the pane after the task exits so output stays readable
workspace = 0                # 0 runs in the current workspace, 1-9 pins it to one
```

`rozi extensions check <path>` lists these with their defaults. An unknown key or a wrong type is
reported and ignored, so the extension always has a usable value.

## How task discovery works

The project root is the nearest directory at or above the pane's working directory containing a
`justfile`, `Makefile`, `package.json`, or `.git` — nearest, so a monorepo package's own tasks win
over the repository root's.

- **just** — recipe names parsed from the justfile. Comments, `export`/assignment lines, and recipe
  bodies are skipped. The `just` binary is not required to list them.
- **make** — target names parsed from the makefile. Variable assignments (`x := y`), `.PHONY` and
  other dot-targets, and recipe lines are skipped.
- **package.json** — the `scripts` object. The runner is `pnpm` or `yarn` when the matching lockfile
  is present, otherwise `npm`.

Parsing rather than shelling out means the list appears without running any project code, which
matters for a sidebar tab that refreshes on a timer.

## State

The last task is remembered in `$XDG_STATE_HOME/rozi-tasks/last-task.json` (or
`~/.local/state/rozi-tasks/`), never inside the installed extension directory.

## Tests

```sh
cd tasks && python -m unittest discover -s tests
```

Covers discovery for each source, root resolution, source ordering, malformed input, and settings
fallback.

## Manual check

```sh
tmp=$(mktemp -d)
printf 'build:\n\techo built\ntest:\n\techo tested\n' > "$tmp/Makefile"
printf '{"scripts":{"dev":"echo dev"}}\n' > "$tmp/package.json"
cd "$tmp"
rozi run-action tasks.run
```

1. The picker shows `build` and `test` under **make**, `dev` under **package.json**.
2. `Enter` on `build` opens a focused pane in `$tmp` running it, held open after it exits.
3. `rozi run-action tasks.repeat` reopens the same task.
4. Open the **Tasks** sidebar tab and confirm the same two sections appear as headers.
5. `rm -rf "$tmp"` when finished.
