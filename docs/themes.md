# Themes

`hyprmux` themes both its own chrome (workbar, borders, titlebars, overlays) and the ANSI
color palette used to render terminal content. Themes come from `tui-lipan`'s `Theme` type:
a built-in preset, the host-derived `system` theme, or a custom theme file.

## Selecting a theme

There is a single knob. `[theme].name` is the active theme, chosen by name:

```toml
[theme]
name = "tokyo-night"
```

The name resolves in this order:

1. A custom theme file `~/.config/hyprmux/themes/<name>.toml`, if it exists (custom files
   shadow built-ins of the same name).
2. The reserved name `system` (derive colors from the host terminal).
3. A [built-in preset](#built-in-presets) id.
4. Otherwise `hyprmux` warns and uses `lipan`.

Pick a theme at runtime with the *Choose theme* palette command. It lists `System`, the
built-in presets, and every custom theme file in one fuzzy-searchable modal; the active theme
is marked `current`. Selecting one applies it immediately and writes `name` back to the config.

## Built-in presets

| Preset id | Label |
| --- | --- |
| `lipan` (default) | Lipan |
| `one-dark` | One Dark |
| `dracula` | Dracula |
| `nord` | Nord |
| `gruvbox` | Gruvbox |
| `catppuccin` | Catppuccin |
| `tokyo-night` | Tokyo Night |
| `solarized-dark` | Solarized Dark |
| `monokai` | Monokai |
| `ansi` | ANSI (uses the host terminal's own palette) |

Preset ids are case-insensitive and accept a few aliases (e.g. `onedark`, `tokyonight`,
`solarized`).

## Custom theme files

Drop as many `tui-lipan` theme TOML files as you like into `~/.config/hyprmux/themes/`. Each
file is a theme named by its stem, so `~/.config/hyprmux/themes/my-nord.toml` is selected with:

```toml
[theme]
name = "my-nord"
```

Custom themes appear in the *Choose theme* picker alongside the built-ins. If a file cannot be
read or parsed, `hyprmux` falls back to `lipan` and reports a warning (the file is still
watched, so fixing it hot-reloads without a restart).

### Hot reload

While a custom theme is active, `hyprmux` watches its file and **hot-reloads** it on every
change (powered by `tui-lipan`'s `theme-reload` feature and `ThemeWatcher`). Edits are picked
up live - chrome and terminal colors update without restarting. Reload errors are surfaced as
toasts. Built-in presets and `system` have no backing file and so do not hot-reload; switch
between them with the picker instead.

## Terminal colors follow the theme

Terminal content is not painted with a single background color. `hyprmux` derives a full
16-color ANSI palette (plus default foreground/background) from the active theme and applies
it to every pane's terminal screen. The mapping uses the theme's status colors (error /
success / warning / info), accent, and file-icon colors for the base 8, with lightened
variants for the bright 8.

This palette is re-applied whenever the theme changes - on a picker selection, on a custom
theme hot-reload, at startup, and to every newly spawned pane - so terminal colors stay in
sync with the chrome.

The `ansi` preset and `system` are the exceptions: they lean on the host terminal's own
palette rather than a curated one, which is useful if you want `hyprmux` to blend into your
existing terminal theme.
