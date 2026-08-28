# Configuration

Rozi reads one TOML file. Every key is optional. Unknown keys and invalid TOML reject the whole
file, load defaults, and produce a warning toast.

The complete inert example is [`examples/config.toml`](../examples/config.toml). Uncomment only the
settings you need.

## File location

Rozi chooses the file in this order:

1. `--config <PATH>`, which also sets `ROZI_CONFIG` for the process.
2. `ROZI_CONFIG`. `~` and `~/...` expand to the home directory.
3. `$XDG_CONFIG_HOME/rozi/config.toml`, or `~/.config/rozi/config.toml`.
4. `%APPDATA%\rozi\config.toml` on Windows.

`--config` applies to launches, session servers, extension inspection, and session lifecycle
commands that load configuration. Control commands do not load configuration and reject
`--config`.

## User directories

| Purpose | Linux and macOS | Windows |
| --- | --- | --- |
| Config | `$XDG_CONFIG_HOME/rozi`, else `~/.config/rozi` | `%APPDATA%\rozi` |
| Data, including extensions | `$XDG_DATA_HOME/rozi`, else `~/.local/share/rozi` | `%LOCALAPPDATA%\rozi` |
| State | `$XDG_STATE_HOME/rozi`, else `~/.local/state/rozi` | `%LOCALAPPDATA%\rozi` |
| Cache | `$XDG_CACHE_HOME/rozi`, else `~/.cache/rozi` | `%LOCALAPPDATA%\rozi\cache` |
| Runtime endpoints | `$XDG_RUNTIME_DIR/rozi`, else a private per-user temporary directory | `%LOCALAPPDATA%\rozi\run` |

Relative `XDG_*` values are ignored. Rozi requires absolute roots.

## Reloading and editing

Rozi watches the config file and applies changes without replacing panes or workspaces. The
`reload-config` action also reloads the file and rescans extensions:

```bash
rozi run-action reload-config
```

Most settings apply immediately. These settings have narrower behavior:

| Setting | When it takes effect |
| --- | --- |
| `shell`, `shell_integration.mode`, `cwd`, `environment.forward` | New panes only. |
| `command_shell` | New command, hook, service, sidebar, and workbar executions. |
| `scrollback` | New terminal screens. Restart an existing session server before creating panes that should use the new capacity. Existing screens never resize. |
| `frame_rate` | Next client launch or reattach. |
| `clipboard.enable_osc52` | Next client launch. |
| `sidebar.visible` | Client startup only. Reload never opens or closes the sidebar. |
| `session.startup` | Next bare launch. |
| `session.resurrect`, `session.allow_takeover` | Session servers started after the change. |
| `logging.*` | Session servers started after the change. |
| `remote.*` | New SSH connections. |
| `rules` | New command-carrying pane spawns. |
| `services` | Reload reconciles definitions. Changed services restart, removed services stop, and unchanged services continue. |
| `agents` | Reload updates detection in the controlled session server and the local scratch session. |
| `extensions.disabled` and extension manifests | Explicit reload. Rozi does not watch extension directories. |

Settings changed in Rozi's own Settings, Appearance, Profiles, or Themes UI are written to the file
and are already active.

The `open-config` action opens the selected file with `EDITOR`, then `VISUAL`, then `vi`:

```bash
rozi run-action open-config
```

## Minimal example

```toml
cwd = "~/code"

[input]
modifier = "super"
modifier_shortcuts = false

[layout]
default = "columns"

[theme]
name = "lipan"
```

## Top-level keys

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `shell` | string or string array | Platform shell | A string is one program with no arguments. An array preserves argv. Unix uses `SHELL`, then `/bin/sh`. Windows tries `pwsh.exe`, `powershell.exe`, `COMSPEC`, then `cmd.exe`. |
| `command_shell` | string or string array | `["/bin/sh", "-c"]` on Unix, `[COMSPEC, "/D", "/S", "/C"]` on Windows | Runs command strings for panes, popups, hooks, services, workbar and sidebar commands, and config commands. |
| `cwd` | path string | Launch directory | `~` expands. Used by new panes. |
| `scrollback` | integer | `5000` | Minimum `1`. |
| `frame_rate` | integer | `120` | Clamped to `15..=480` with a warning. |
| `nerd_icons` | bool | `true` | Enables private-use glyphs in chrome. File icons also require a sidebar tree tab with `icons = true`. |

New local panes receive `ROZI=1`, `ROZI_PANE`, and, when available, `ROZI_SOCKET` and `ROZI_BIN`.
See [Scripting](scripting.md) and [Control CLI](control.md).

## `[shell_integration]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `mode` | string | `"auto"` | `"auto"` or `"off"`. Auto-injects OSC 7 and OSC 133 support into recognized interactive shells without editing shell startup files. |

See [Terminal features](terminal.md#working-directories-and-shell-metadata).

## `[environment]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `forward` | array of strings | `[]` | Names of extra client environment variables copied to new local panes. Empty names are removed and duplicates are collapsed. Values are not persisted or forwarded through remote attachments. |

Rozi already forwards the current desktop session variables needed by Wayland, X11, D-Bus, and
Hyprland. Existing panes keep their original environment.

## `[input]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `modifier` | string | `"alt"` | `"alt"` or `"super"`. `mod` aliases Alt. `meta`, `logo`, `win`, and `windows` alias Super. |
| `prefix` | string | `"ctrl-a"` | One valid tui-lipan key step. |
| `modifier_shortcuts` | bool | `true` | Mirrors generated prefix bindings onto held modifier chords. |
| `which_key` | string | `"short"` | `"off"`, `"instant"`, `"short"` at 300 ms, or `"long"` at 750 ms. |

See [Keybindings](keybindings.md).

### The which-key strip

The strip is documented in [Keybindings](keybindings.md#prefix-and-held-modifier).

## `[layout]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `split_width_multiplier` | float | `2.3` | Must be finite and greater than zero. |
| `default` | string | `"dwindle"` | `"dwindle"`, `"master"`, `"grid"`, `"columns"`, `"rows"`, `"scrollable"`, or `"monocle"`. Profiles may override it per workspace. |

See [Layouts and panes](layouts-and-panes.md).

## `[pane]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `resize_debounce_ms` | integer | `16` | Minimum delay between PTY resize batches. `0` forwards each report. |
| `focus_on_hover` | bool | `true` | Focuses a pane when the pointer enters it. |
| `hold_on_exit` | bool | `false` | Retains naturally exited shell panes. Command panes use their `keep_open` value. |
| `highlight_focused_background` | bool | `false` | Uses the panel background for the focused pane. |
| `highlight_focused_border` | bool | `true` | Uses the active border color for the focused pane. |
| `highlight_focused_titlebar` | bool | `true` | Uses focused titlebar styling. |
| `show_workbar` | bool | `true` | Shows the workbar. |
| `workbar_gap` | bool | `true` | Keeps one row between the workbar and panes. |
| `workbar_at_bottom` | bool | `false` | Places the workbar below panes. |
| `show_titles` | bool | `true` | Shows pane titles without changing `titlebar`. |
| `titlebar` | string | `"bar"` | `"bar"`, `"border"`, `"integrated"`, or `"inset"`. |
| `border_mode` | string | `"separate"` | `"separate"`, `"merged"`, `"none"`, or `"dividers"`. |
| `alert_border` | string | `"pulse"` | `"off"`, `"static"`, or `"pulse"`. |
| `border_style` | string | `"rounded"` | `"rounded"`, `"plain"`, `"double"`, or `"thick"`. Applies to framed modes. |
| `keep_special_borders` | bool | `true` | Keeps frames on floating panes, popups, and scratchpads in borderless modes. |
| `padding` | integer or integer array | `0` | One value, `[vertical, horizontal]`, or `[top, right, bottom, left]`. Each side is clamped to `0..=8`. |
| `title_style` | string | `"padded"` | `"padded"`, `"half"`, `"round"`, or `"arrow"`. |
| `workbar_badge_style` | string | `"padded"` | `"padded"`, `"round"`, or `"arrow"`. If `workbar_tab_style` is absent, this also sets tab style. |
| `workbar_tab_style` | string | `workbar_badge_style` | `"padded"`, `"round"`, or `"arrow"`. |
| `workbar_style` | string | `"padded"` | `"padded"`, `"half"`, `"round"`, or `"arrow"`. |
| `workbar_powerline` | bool | `true` | Joins trailing workbar badges. |
| `toast_opacity` | float | `0.8` | Finite value in `0.0..=1.0`. Invalid values are ignored. |
| `background_follows_terminal` | bool | `false` | Uses the host terminal background for the canvas backdrop. |

See [Layouts and panes](layouts-and-panes.md), [Sidebar](sidebar.md), and [Themes](themes.md).

### `[pane.alert]`

Each value is a theme role or `"off"`. Theme roles are `accent`, `info`, `success`, `warning`,
`error`, `neutral`, and `panel`.

| Key | Type | Default |
| --- | --- | --- |
| `blocked` | string | `"error"` |
| `finished` | string | `"success"` |
| `working` | string | `"off"` |
| `idle` | string | `"off"` |

## `[animations]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `true` | Master switch. |
| `spawn` | bool | `true` | Animates pane creation. |
| `close` | bool | `true` | Animates pane close. |
| `fullscreen` | bool | `true` | Animates fullscreen transitions. |
| `tile_float` | bool | `true` | Animates tile and float transitions. |
| `axis_change` | bool | `true` | Animates split-axis changes. |
| `sidebar` | bool | `true` | Animates sidebar movement. |
| `focus_chrome` | bool | `true` | Animates focus color changes and enables alert pulses. |
| `pane_style` | string | `"scale"` | `"scale"` or `"slide"`. |
| `geometry_ms` | integer | `220` | Base geometry duration in milliseconds. |
| `close_ms` | integer | `120` | Scale close duration in milliseconds. |
| `focus_chrome_ms` | integer | `160` | Focus color duration in milliseconds. |
| `alert_pulse_ms` | integer | `1600` | Alert pulse period. Half-period is floored at 400 ms. |
| `open_delay_ms` | integer | `36` | Spawn animation delay in milliseconds. |

### Pane open/close style

See [Layouts and panes](layouts-and-panes.md).

## `[theme]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | `"rozi"` | Built-in theme ID, `"system"`, or a file stem from the themes directory. Custom themes reload while active. |

See [Themes](themes.md).

## `[profile]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `default` | string | none | Default profile used when no explicit recipe has higher precedence. |

See [Profiles](profiles.md).

## `[clipboard]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enable_osc52` | bool | `true` | Allows pane programs to set the system clipboard with OSC 52. Requires a client restart. |

See [Terminal features](terminal.md#select-copy-and-paste).

## `[notifications]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master desktop notification switch. |
| `pane_exit` | bool | `false` | Notifies on clean natural pane exits. |
| `pane_exit_error` | bool | `true` | Notifies on nonzero natural pane exits. |
| `pane_blocked` | bool | `true` | Notifies when an unattended pane becomes blocked. |
| `pane_done` | bool | `false` | Notifies on an unseen working-to-finished transition. |
| `bell` | bool | `true` | Marks an unattended pane urgent on BEL. Independent of `enabled`. |

Desktop notifications use the platform notification implementation and are best effort.

## `[sounds]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `enabled` | bool | `false` | Master sound switch. |
| `bell` | bool | `true` | Enables the bell cue. |
| `blocked` | bool | `true` | Enables the blocked cue. |
| `done` | bool | `true` | Enables the done cue. |
| `error` | bool | `true` | Enables the error cue. |
| `throttle_ms` | integer | `2000` | Clamped to `100..=60000` with a warning. |
| `bell_file` | path string | empty | WAV override. `~` expands. |
| `blocked_file` | path string | empty | WAV override. `~` expands. |
| `done_file` | path string | empty | WAV override. `~` expands. |
| `error_file` | path string | empty | WAV override. `~` expands. |
| `player` | string | empty | Player executable. Rozi appends the cue path as the final argument. |

## `[navigation]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `editors` | array of strings | vim family, Helix, Kakoune, Emacs, and fzf | Replaces the process-name list used by `smart-focus-*`. Empty names are removed. Matching is case-insensitive. |

See [Commands without default keys](keybindings.md#commands-without-default-keys).

## `[confirm]`

These switches apply to shortcuts and `run-action`. Commands chosen from the command palette use
their own deliberate selection path.

| Key | Type | Default |
| --- | --- | --- |
| `close_pane` | bool | `false` |
| `kill_workspace` | bool | `true` |
| `kill_session` | bool | `true` |
| `quit_ephemeral` | bool | `true` |
| `new_temporary_session` | bool | `true` |
| `load_profile` | bool | `true` |

## `[session]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `autosave` | bool | `false` | Saves and restores local layout intent, not live PTYs. |
| `resurrect` | bool | `true` | Saves named-session layout, command, scrollback, and restart intent. |
| `startup` | string | `"picker"` | `"picker"`, `"ephemeral"`, `"last"`, or `"profile"`. |
| `path` | path string | State directory `session.toml` | Autosave file. `~` expands. |
| `allow_takeover` | bool | `true` | Lets a writable follower take layout control immediately. |

See [Sessions](sessions.md).

## `[remote]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `default_host` | string | none | Host used by `--remote` without a value. |
| `connection_timeout_secs` | integer | `15` | SSH `ConnectTimeout`. |
| `server_alive_interval_secs` | integer | `15` | Minimum `1`. |
| `server_alive_count_max` | integer | `3` | Minimum `1`. |
| `install` | string | `"prompt"` | `"prompt"`, `"always"`, or `"never"`. Noninteractive runs never install. |
| `batch_mode` | bool | `true` | Sets SSH `BatchMode=yes`. |

### `[remote.hosts.<alias>]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `host` | string | Alias | SSH hostname. |
| `user` | string | SSH default | Login user. |
| `port` | integer | SSH default | `0` is ignored. |
| `identity_file` | path string | none | SSH identity path. |
| `ssh_args` | array of strings | `[]` | Extra SSH argv. |
| `binary_path` | string | none | Absolute remote Rozi path. Skips probing and installation. |

See [Remote sessions](remote.md).

## `[scratchpad]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `command` | string | Normal shell | Command for the first scratch pane. |
| `cwd` | path string | Focused local pane cwd, then configured `cwd` | `~` expands. Captured when the scratchpad is first created. |
| `height` | float | `0.4` | Clamped to `0.1..=0.9` with a warning. |

See [Popups and scratch panes](layouts-and-panes.md#popups-and-scratch-panes).

## `[sidebar]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `visible` | bool | `false` | Startup visibility only. |
| `width` | integer | `32` | Clamped to `16..=80`. |
| `position` | string | `"left"` | `"left"` or `"right"`. |
| `tabs` | array | `["activity", "panes", "sessions", "files", "git"]` | Replaces the tab catalog. IDs must be unique. |
| `panels` | array of one or two string arrays | `[["activity", "panes", "sessions"], ["files", "git"]]` | Orders tab IDs. Unknown and duplicate IDs are skipped. Omitted configured tabs are appended to the first panel. |
| `split` | bool | Inferred from panel count, `true` by default | Shows two saved panel groups. |
| `split_ratio` | float | `0.4` | Finite value clamped to `0.15..=0.85`. |

A table in `tabs` can configure `files` or `git`, or define a custom launcher or command tab.

| Tab key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | required | Unique tab ID. `activity`, `panes`, and `sessions` are reserved. |
| `label` | string | required for custom tabs | Built-in tree labels are fixed. |
| `entries` | array of tables | none | Launcher rows. Exactly one of `entries` or `command` is required for a custom tab. |
| `command` | string | none | Command-tab producer. |
| `interval` | integer seconds | `30` | Minimum `5`. Command tabs only. |
| `on_click` | action table | none, except tree tabs type `{path}` | Action for a command or tree row. |
| `root` | string | `"cwd"` for files, `"repo"` for git | `"cwd"` or `"repo"`. Tree tabs only. |
| `show_hidden` | bool | `true` | Tree tabs only. |
| `icons` | bool | `false` | Tree tabs only. Also requires `nerd_icons`. |
| `explorer` | bool | `false` | Tree tabs only. |
| `diff_stats` | bool | `false` for files, `true` for git | Tree tabs only. |
| `max_entries` | integer | `2000` | Clamped to `1..=10000`. Tree tabs only. |

Launcher entries use `label`, exactly one of `run`, `send`, or `popup`, and optional `keep_open`
which defaults to `true`. An `on_click` action accepts `label`, exactly one of `run`, `send`,
`popup`, or `exec`, and optional `keep_open`. `label` only affects command presentation.

### Opening a diff viewer or editor from a row

Tree `send` actions may substitute `{path}` because the result is literal PTY input. Never append a
newline unless you intend to execute the selected text.

Tree `run`, `popup`, and `exec` actions receive the selected path in `ROZI_FILE`. `run` and `popup`
reject `{path}`, and `exec` does not expand it. Quote the environment expansion for the configured
command shell:

```toml
[sidebar]
tabs = [
  "activity",
  { name = "files", label = "", on_click = { run = '''"${EDITOR:-vi}" "$ROZI_FILE"''' } },
  { name = "git", label = "", on_click = { popup = '''git diff -- "$ROZI_FILE"''', keep_open = false } },
]
```

No selected path is inserted into a command string. See [Sidebar files](sidebar.md#files).

## `[workbar]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `left` | segment array | `["title", "workspaces"]` | Ordered left region. |
| `right` | segment array | `["location", "session"]` | Ordered right region. |
| `clock_format` | string | `"%H:%M"` | Valid strftime format. Invalid formats are ignored. |

A segment is a string or `{ segment = "...", color = "..." }`. Colors are `accent`, `info`,
`success`, `warning`, `error`, `neutral`, or `panel`.

Segment names are `title`, `workspaces`, `location`, `session`, `clock`, `layout`, `activity`,
`text:<literal>`, `command:<shell command>`, and `command:<interval seconds>:<shell command>`.
Text segments support `{host}`, `{workspace}`, `{layout}`, and `{session}`. Command segments refresh
every 60 seconds by default, use a minimum interval of 1 second, time out after 5 seconds, and
capture at most 64 KiB per output stream.

### `[workbar.alert]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `bell` | bool | `true` | Marks workspaces with a bell. |
| `blocked` | bool | `true` | Marks blocked workspaces. |
| `finished` | bool | `true` | Marks unseen finished workspaces. |
| `working` | bool | `false` | Marks working workspaces. |
| `idle` | bool | `false` | Marks idle workspaces. |
| `mode` | string | `"pulse"` | `"off"`, `"static"`, or `"pulse"`. |
| `paint` | string | `"background"` | `"background"` or `"text"`. |

## `[logging]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `dir` | path string | State directory `logs` | `~` expands. |
| `max_bytes` | integer | `67108864` | Per-file limit. `0` allows unbounded growth. |

See [Pane logging](terminal.md#pane-logging).

## `[[rules]]`

Rules apply in declaration order to new ordinary panes with an explicit command. The first match
wins.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `match` | string | none | Case-sensitive substring. Set exactly one matcher. |
| `match_regex` | string | none | `regex-lite` pattern. Set exactly one matcher. |
| `float` | bool | `false` | Opens a floating pane. |
| `width` | float | `0.6` when floating | Clamped to `0.1..=1.0`. |
| `height` | float | `0.6` when floating | Clamped to `0.1..=1.0`. |
| `position` | string | `"center"` | `center`, `cursor`, `top-left`, `top`, `top-right`, `left`, `right`, `bottom-left`, `bottom`, or `bottom-right`. Ignored unless floating. |
| `workspace` | integer | Current workspace | `1..=9`. Invalid values are ignored. |
| `focus` | bool | `true` | Focuses the pane and its workspace. |
| `fullscreen` | bool | `false` | Starts fullscreen. |

Control `new-pane --workspace` and `--focus` override those two rule fields. See
[Layouts and panes](layouts-and-panes.md).

## `[[agents]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `id` | string | required | Lowercase letters, digits, `_`, and `-`. A config entry with a built-in ID replaces that built-in. |
| `label` | string | ID | Activity label. |
| `base` | bool | `true` | Enables shared state patterns. |
| `match.names` | array of strings | `[]` | Executable basenames. |
| `match.paths` | array of strings | `[]` | Lowercase path or argv substrings. |
| `states` | array of state tables | `[]` | State rules. |

A new definition needs at least one match name or path. A config definition that replaces a
built-in may omit `match` and inherit the built-in process match.

| State key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `state` | string | required | `"unknown"`, `"blocked"`, `"working"`, or `"idle"`. Evaluation uses that precedence, not declaration order. |
| `scope` | string | `"all"` | `"all"` or `"footer"`. Footer reads the last eight nonempty screen lines. |
| `screen` | pattern table | none | Set exactly one of `screen` or `title`. |
| `title` | pattern table | none | Set exactly one of `screen` or `title`. `scope` does not apply. |

Pattern tables accept `all_of`, `any_of`, and `none_of` string arrays plus `regex`, a bool that
defaults to `false`. At least one of `all_of` or `any_of` is required. Matching reads lowercase
text. Invalid rules are skipped. An invalid definition is dropped.

See [Agent definitions](agents.md).

## `[[hints]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `pattern` | string | required | Nonempty `regex-lite` pattern. Invalid patterns are skipped. |
| `open` | bool | `false` | Lets the uppercase hint label open the match. |

Built-in URL, path, and Git SHA hints run first and win overlaps. See [Terminal features](terminal.md).

## `[[hooks]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `event` | string | required | Public event ID. Unknown IDs are skipped. |
| `run` | string | required | Nonempty command string run through `command_shell`. |

Multiple hooks may use the same event. See [Hooks](hooks.md).

## `[[commands]]`

Named commands have stable IDs and can be invoked with `rozi run-action`.

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `id` | string | required | Lowercase letters, digits, `_`, and `-`. Dots, built-in IDs, and reserved prefixes are rejected. |
| `label` | string | Generated | Palette and help label. |
| `run` | string | none | Opens a pane through `command_shell`. |
| `send` | string | none | Sends literal text to the focused PTY. |
| `popup` | string | none | Opens a centered popup through `command_shell`. |
| `exec` | string | none | Runs detached through `command_shell`, discarding output. |
| `keep_open` | bool | `true` | Applies to `run` and `popup`. |

Exactly one of `run`, `send`, `popup`, or `exec` is required.

```toml
[[commands]]
id = "branches"
label = "Switch branch"
exec = "~/.config/rozi/branch-pick.sh"

[keys]
branches = "i"
```

## `[extensions]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `disabled` | array of strings | `[]` | Stable manifest IDs to disable. Directory names are not extension IDs. |

See [Extensions](extensions.md).

## `[[services]]`

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `name` | string | required | Nonempty and unique. |
| `run` | string | required | Nonempty command string run through `command_shell`. |
| `cwd` | path string | Launch directory | `~` expands when the service starts. |
| `restart` | string | `"on-failure"` | `"on-failure"`, `"always"`, or `"never"`. |
| `env` | string table | `{}` | Child environment overrides. |

Services receive `ROZI=1`, `ROZI_SERVICE`, `ROZI_BIN`, and `ROZI_SOCKET` when control is available.
They use a 1, 2, 4, 8, 16, then 30 second restart backoff. Five consecutive failures inside 60
seconds make a service dormant until its definition changes. Rozi terminates service process groups
when the client exits.

Use extension services for packaged automation. See [Extensions](extensions.md).

## `[keys]`

The key is either a built-in action ID, a named command ID, an extension command ID, or a trigger
for an inline command.

### Action and named-command bindings

| Value form | Behavior |
| --- | --- |
| `"b"` or `["b", "super-enter"]` | Replaces defaults. A bare key expands through the prefix and modifier scheme. A literal chord stays literal. |
| `"scheme:ctrl-t"` | Expands one modified key through the prefix and modifier scheme. |
| `{ add = "super-enter" }` | Adds one binding without removing defaults. `add` also accepts an array. |
| `""` or `[]` | Removes all bindings for that action. |

Comma-separated alternatives are accepted inside strings. If every nonempty replacement candidate
is invalid, Rozi keeps the action defaults.

See [Keybindings](keybindings.md) for action IDs and key syntax.

### User-defined command keybindings

An inline command table uses these keys:

| Key | Type | Default | Constraints and behavior |
| --- | --- | --- | --- |
| `label` | string | Generated | Palette and help label. |
| `run` | string | none | Opens a pane. |
| `send` | string | none | Sends literal PTY text. |
| `popup` | string | none | Opens a popup. |
| `exec` | string | none | Runs detached and discards output. |
| `keep_open` | bool | `true` | Applies to `run` and `popup`. |

Exactly one action is required. Inline commands do not have stable action IDs and cannot be called
with `run-action`.

```toml
[keys]
g = { run = "lazygit", label = "Git UI", keep_open = false }
"ctrl-a e" = { send = "ls -la\n" }
u = { exec = "rozi run-action toggle-float", label = "Float pane" }
```
