# Themes

`hyprmux` themes both its own chrome (top bar, borders, titlebars, overlays) and the ANSI
color palette used to render terminal content. Themes come from `tui-lipan`'s `Theme` type:
either a built-in preset or a custom theme file.

## Built-in presets

Set a preset in the config, or pick one at runtime with the *Choose theme* palette command
(which opens a list modal; the current built-in is marked with a check).

| Preset id | Label |
| --- | --- |
| `one-dark` (default) | One Dark |
| `dracula` | Dracula |
| `nord` | Nord |
| `gruvbox` | Gruvbox |
| `catppuccin` | Catppuccin |
| `tokyo-night` | Tokyo Night |
| `solarized-dark` | Solarized Dark |
| `monokai` | Monokai |
| `ansi` | ANSI (uses the host terminal's own palette) |

```toml
[theme]
preset = "tokyo-night"
```

Preset ids are case-insensitive and accept a few aliases (e.g. `onedark`, `tokyonight`,
`solarized`). Choosing a theme from the picker applies it for the session and clears any
custom theme file.

## Custom theme files

Point `[theme].path` at a `tui-lipan` theme TOML file:

```toml
[theme]
preset = "one-dark"        # fallback if the file fails to load
path = "~/.config/hyprmux/theme.toml"
```

The custom file is loaded at startup. If it cannot be read or parsed, `hyprmux` falls back to
the `preset` theme and reports a startup warning.

### Hot reload

When a custom theme `path` is set, `hyprmux` watches the file and **hot-reloads** it on every
change (powered by `tui-lipan`'s `theme-reload` feature and `ThemeWatcher`). Edits are picked
up live — chrome and terminal colors update without restarting. Reload errors are surfaced as
toasts.

## Terminal colors follow the theme

Terminal content is not painted with a single background color. `hyprmux` derives a full
16-color ANSI palette (plus default foreground/background) from the active theme and applies
it to every pane's terminal screen. The mapping uses the theme's status colors (error /
success / warning / info), accent, and file-icon colors for the base 8, with lightened
variants for the bright 8.

This palette is re-applied whenever the theme changes — on a picker selection, on a custom
theme hot-reload, at startup, and to every newly spawned pane — so terminal colors stay in
sync with the chrome.

The `ansi` preset is the exception: it leans on the host terminal's own palette rather than a
curated one, which is useful if you want `hyprmux` to blend into your existing terminal theme.
