# Themes

`rozi` themes both its own chrome (workbar, borders, titlebars, overlays) and the ANSI
color palette used to render terminal content. Themes come from `tui-lipan`'s `Theme` type:
a built-in preset, the host-derived `system` theme, or a custom theme file.

## Selecting a theme

There is a single knob. `[theme].name` is the active theme, chosen by name:

```toml
[theme]
name = "tokyo-night"
```

The name resolves in this order:

1. A custom theme file `~/.config/rozi/themes/<name>.toml`, if it exists (custom files
   shadow built-ins of the same name).
2. The reserved name `system` (derive colors from the host terminal).
3. A [built-in preset](#built-in-presets) id.
4. Otherwise `rozi` warns and uses `lipan`.

Pick a theme at runtime with the *Choose theme* palette command. It groups `System`, dark
presets, light presets, and every custom theme file in one fuzzy-searchable modal; the active
theme is marked `current`. Selecting one applies it immediately and writes `name` back to the
config.

## Built-in presets

| Preset id | Label |
| --- | --- |
| `lipan` (default) | Lipan |
| `one-dark` | One Dark |
| `dracula` | Dracula |
| `nord` | Nord |
| `gruvbox-dark` | Gruvbox Dark |
| `catppuccin-mocha` | Catppuccin Mocha |
| `tokyo-night` | Tokyo Night |
| `solarized-dark` | Solarized Dark |
| `monokai` | Monokai |
| `solarized-light` | Solarized Light |
| `gruvbox-light` | Gruvbox Light |
| `tokyo-night-day` | Tokyo Night Day |
| `catppuccin-latte` | Catppuccin Latte |
| `catppuccin-frappe` | Catppuccin Frappe |
| `catppuccin-macchiato` | Catppuccin Macchiato |
| `rose-pine` | Rosé Pine |
| `rose-pine-moon` | Rosé Pine Moon |
| `rose-pine-dawn` | Rosé Pine Dawn |
| `kanagawa` | Kanagawa |
| `everforest` | Everforest |
| `ayu-dark` | Ayu Dark |
| `ayu-mirage` | Ayu Mirage |
| `ayu-light` | Ayu Light |
| `nightfox` | Nightfox |
| `nordfox` | Nordfox |
| `night-owl` | Night Owl |
| `material-palenight` | Material Palenight |
| `oxocarbon` | Oxocarbon |
| `zenburn` | Zenburn |

Preset ids are case-insensitive and accept a few aliases (e.g. `onedark`, `tokyonight`,
`solarized`, `gruvbox`, and `catppuccin`). The latter two aliases resolve to the renamed
`gruvbox-dark` and `catppuccin-mocha` presets.

`ansi` remains a valid config value for existing configurations, but is not shown in the picker.
When `system` cannot query the host terminal's colors, rozi uses ANSI colors for that run and
shows a warning. The configured name remains `system`, so probing is retried on the next launch.

## Custom theme files

Drop as many `tui-lipan` theme TOML files as you like into `~/.config/rozi/themes/`. Each
file is a theme named by its stem, so `~/.config/rozi/themes/my-nord.toml` is selected with:

```toml
[theme]
name = "my-nord"
```

Custom themes appear in the *Choose theme* picker alongside the built-ins. If a file cannot be
read or parsed, `rozi` falls back to `lipan` and reports a warning (the file is still
watched, so fixing it hot-reloads without a restart).

### Inheriting from a preset with `extends`

A custom theme file does not need to define every color. Add `extends` at the top of the file
to start from one of the built-in presets, then override only the fields you care about -
everything else is inherited from that preset:

```toml
# ~/.config/rozi/themes/my-nord.toml
extends = "nord"

[accent]
fg = "#ff79c6"
```

`extends` accepts any built-in preset id (see [Built-in presets](#built-in-presets)), plus
`lipan`. It is optional; a file with no `extends` inherits from `lipan` instead.

Every top-level `Theme` field can be overridden the same way, by name, and only the sub-fields
you set are applied on top of the base - the rest of that field's value is inherited:

- Style fields: `primary`, `accent`, `selection`, `text_selection`, `focus`, `hover`, `border`,
  `muted`. Each is a table of `fg`, `bg`, `bold`, `dim`, `italic`, `underline`, `reverse`,
  `strikethrough`, `underline_color`, `dim_amount`, and `tint`.
- Palette fields: `surface`, `status`, `file_icons`, `git_status`, `diff`, `document`, `syntax`,
  `input`, `text_area`, `document_view`, `hex_area`, `terminal`, `scrollbar`, `splitter`.
- The single color `border_active`.

Colors accept hex (`"#RRGGBB"`), ANSI names (`"cyan"`, `"darkgray"`, ...), `indexed(<0-255>)`,
or `rgb(r,g,b)`. Style-table `fg`/`bg`/`underline_color` fields additionally accept alpha via
`"#RRGGBBAA"` or `rgba(r,g,b,a)` (alpha as `0.0..=1.0` or `0..=255`); bare palette fields such as
`status` and `surface` are opaque-only.

For example, a theme that keeps Dracula everywhere but swaps only the success/error status
colors and brightens the active border. `status.error` and `status.success` also color blocked and
finished pane alerts; an unreadable alert role falls back through `status.warning` to readable text:

```toml
extends = "dracula"
border_active = "#f8f8f2"

[status]
success = "#50fa7b"
error = "#ff5555"
```

### Matching the host terminal's background

The easiest way to get this is the *Background follows terminal* toggle in Settings
picker (`[pane] background_follows_terminal` in config, off by default). It pins
`surface.backdrop` to the host terminal's background for whichever theme is active, without
needing a custom theme file at all - see [`[pane]`](configuration.md#pane) in the configuration
reference.

The rest of this section covers doing the same thing by hand in a custom theme file, which is
useful if you only want it for one specific theme rather than every theme you switch to.

The app's main background (the canvas behind panes and gaps, plus the fill behind unfocused
pane frames) is `surface.backdrop`. Set it to the special value `"backdrop"` to make it inherit
your terminal emulator's own background live, instead of a fixed color:

```toml
extends = "nord"

[surface]
backdrop = "backdrop"
```

`"backdrop"` is a sentinel, not a color. `rozi` resolves it once, at startup, to the actual
background color it queries from your terminal emulator (the same probe the `system` theme uses),
so every surface that reads `surface.backdrop` tracks your terminal's real background instead of a
fixed theme color. When the query is unavailable (some terminals or headless runs), it falls back
to the theme's `surface.panel`.

Resolving to a concrete color - rather than leaving the channel unset - is deliberate: the
backdrop also feeds the terminal default background reported to `OSC 11` background queries,
embedded panes' default-background cells, and workbar badge text and end caps. A bare unset
sentinel has no RGB, so those consumers used to collapse to pitch black (black pane spawns,
black-based transitions in nested apps, unreadable workbar text). A queried color keeps all of
them on your terminal's own background.

Because the value is a color snapshot taken at startup, a live wallpaper or blur behind your
terminal is matched by color, not shown through the panes themselves. Switching your terminal
theme at runtime does not re-probe; reload or restart `rozi` to pick up the new background.

### Hot reload

While a custom theme is active, `rozi` watches its file and **hot-reloads** it on every
change (powered by `tui-lipan`'s `theme-reload` feature and `ThemeWatcher`). Edits are picked
up live - chrome and terminal colors update without restarting. Reload errors are surfaced as
toasts. Built-in presets and `system` have no backing file and so do not hot-reload; switch
between them with the picker instead.

## Terminal colors follow the theme

Terminal content is not painted with a single background color. `rozi` derives a full
16-color ANSI palette (plus default foreground/background) from the active theme and applies
it to every pane's terminal screen. The mapping uses the theme's status colors (error /
success / warning / info), accent, and file-icon colors for the base 8, with lightened
variants for the bright 8.

This palette is re-applied whenever the theme changes - on a picker selection, on a custom
theme hot-reload, at startup, and to every newly spawned pane - so terminal colors stay in
sync with the chrome.

The `ansi` preset and `system` are the exceptions: they lean on the host terminal's own
palette rather than a curated one, which is useful if you want `rozi` to blend into your
existing terminal theme.
