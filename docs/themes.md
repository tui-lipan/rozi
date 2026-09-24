# Themes

A theme sets the colors of rozi's interface and the ANSI palette that programs in panes use. This
page covers choosing a built-in theme, following the host terminal's colors, and writing your own
theme file.

## Choose a built-in theme

Open **Settings**, choose **Theme**, and select a theme. rozi previews the highlighted theme as you
move through the list. Selecting one writes `[theme].name` to `config.toml`.

You can also set it directly:

```toml
[theme]
name = "tokyo-night"
```

<CaptureGallery title="~/src/rozi — dev">
<img src="./assets/captures/workspace.webp" alt="A rozi workspace in the default rozi theme with Neovim, lazygit, btop, and a shell" data-label="rozi" data-code='[theme]\nname = "rozi"'>
<img src="./assets/captures/theme-catppuccin-mocha.webp" alt="The same workspace in the Catppuccin Mocha theme" data-label="catppuccin-mocha" data-code='[theme]\nname = "catppuccin-mocha"'>
<img src="./assets/captures/theme-tokyo-night.webp" alt="The same workspace in the Tokyo Night theme" data-label="tokyo-night" data-code='[theme]\nname = "tokyo-night"'>
<img src="./assets/captures/theme-gruvbox-dark.webp" alt="The same workspace in the Gruvbox Dark theme" data-label="gruvbox-dark" data-code='[theme]\nname = "gruvbox-dark"'>
<img src="./assets/captures/theme-nord.webp" alt="The same workspace in the Nord theme" data-label="nord" data-code='[theme]\nname = "nord"'>
<img src="./assets/captures/theme-rose-pine-dawn.webp" alt="The same workspace in the light Rose Pine Dawn theme" data-label="rose-pine-dawn" data-code='[theme]\nname = "rose-pine-dawn"'>
</CaptureGallery>

Programs in panes that use the 16 ANSI colors, as most do, follow the theme too.

| Id | Label |
| --- | --- |
| `rozi` | Rozi, the default |
| `lipan` | Lipan |
| `one-dark` | One Dark |
| `dracula` | Dracula |
| `nord` | Nord |
| `gruvbox-dark` | Gruvbox Dark |
| `gruvbox-light` | Gruvbox Light |
| `catppuccin-mocha` | Catppuccin Mocha |
| `catppuccin-frappe` | Catppuccin Frappe |
| `catppuccin-macchiato` | Catppuccin Macchiato |
| `catppuccin-latte` | Catppuccin Latte |
| `tokyo-night` | Tokyo Night |
| `tokyo-night-day` | Tokyo Night Day |
| `solarized-dark` | Solarized Dark |
| `solarized-light` | Solarized Light |
| `monokai` | Monokai |
| `rose-pine` | Rose Pine |
| `rose-pine-moon` | Rose Pine Moon |
| `rose-pine-dawn` | Rose Pine Dawn |
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

Built-in theme ids are case-insensitive, and underscores and spaces count as hyphens. Common short
forms such as `onedark`, `tokyonight`, `gruvbox`, `catppuccin`, and `solarized` also work.

`ansi` is a valid config value that uses the host terminal's ANSI colors. It is not shown in the
picker.

An unknown theme name falls back to `rozi` with a warning.

## Use the host terminal colors

Set `name = "system"` to build the theme from the host terminal's foreground, background, and ANSI
colors. If the terminal does not answer rozi's color query, rozi warns and uses `ansi` for that
run. `system` stays configured, and rozi tries again at the next launch.

To keep your chosen theme but use the host terminal's background behind panes, turn on
**Panes › Background › Follows terminal** in Settings, or set:

```toml
[pane]
background_follows_terminal = true
```

rozi watches the host terminal's colors while it runs. When you change the host terminal's theme,
the foreground, background, and all 16 ANSI colors update without restarting rozi.

The sidebar normally uses the theme's raised element color. To give it the same backdrop as the
panes instead, turn on **Bars › Sidebar › Background follows canvas** in Settings, or set:

```toml
[sidebar]
background_follows_canvas = true
```

## Create a custom theme

1. Create a TOML file in the `themes` directory inside your config directory:
   `~/.config/rozi/themes/` (or `$XDG_CONFIG_HOME/rozi/themes/`) on Linux and macOS, and
   `%APPDATA%\rozi\themes\` on Windows. The filename without `.toml` is the theme id.
2. Pick a starting theme with `extends`, and override only what you want to change:

   ```toml
   # ~/.config/rozi/themes/my-nord.toml
   extends = "nord"

   [accent]
   fg = "#ff79c6"

   [status]
   success = "#50fa7b"
   error = "#ff5555"
   ```

3. Select `my-nord` in **Settings › Theme**, or set `name = "my-nord"` under `[theme]`.

A custom file with the same id as a built-in theme, or named `system`, replaces it.

`extends` is optional; without it, the theme starts from `lipan`. It accepts every built-in theme
except `rozi`, plus `ansi`. It does not accept `system`. Matching ignores case, hyphens,
underscores, and spaces.

While a custom theme is active, rozi reloads it whenever the file changes. If the file has a parse
error, rozi warns and uses `lipan` until you fix it.

## Theme file reference

A custom theme lists only the fields it changes. Every omitted field keeps its value from
`extends`.

### Top-level fields

The style tables are `primary`, `accent`, `selection`, `text_selection`, `focus`, `hover`, `border`,
and `muted`. Each accepts the [style fields](#style-fields) below.

`primary` is ordinary text. `accent` covers focus, active state, and highlights. Headers, directory
names, and section titles also use the `accent` color.

The other top-level fields are:

| Field | Shape |
| --- | --- |
| `extends` | preset id |
| `focus_decoration` | boolean |
| `border_active` | color |
| `caret` | `shape`, `color` |
| `surface` | `panel`, `element`, `menu`, `backdrop` |
| `status` | `success`, `warning`, `error`, `info` |
| `file_icons` | `azure`, `blue`, `cyan`, `green`, `grey`, `orange`, `purple`, `red`, `yellow` |
| `git_status` | `modified`, `added`, `deleted`, `renamed`, `untracked`, `conflicted` |
| `diff` | `context`, `added`, `removed`, `empty`, `added_word`, `removed_word`, `added_marker`, `removed_marker`, `context_line_number`, `added_line_number`, `removed_line_number`, `context_separator_style`, `patch_header` |
| `document` | `heading_styles`, `code_inline`, `code_block`, `emphasis`, `strong`, `strikethrough`, `link`, `blockquote_bar`, `table_border`, `table_header`, `hr`, `list_item`, `list_enumeration`, `diagram_node_fill_style`, `diagram_node_border_style`, `diagram_node_label_style`, `diagram_edge_style`, `diagram_muted_style` |
| `syntax` | `comment`, `keyword`, `string`, `number`, `constant`, `function`, `builtin`, `type_name`, `variable`, `parameter`, `operator` |
| `input` | `focus` style |
| `text_area` | `focus` style |
| `document_view` | `focus` style |
| `hex_area` | `focus`, `cursor` styles |
| `terminal` | `focus` style |
| `scrollbar` | `track`, `thumb`, `thumb_focus` colors |
| `splitter` | `hover`, `active` colors |

`heading_styles` is an array of six style tables. Caret `shape` is `block`, `bar`, or `underline`.

### Style fields

Every style table accepts:

- `fg`, `bg`, and `underline_color`
- `bold`, `dim`, `italic`, `underline`, `reverse`, and `strikethrough`
- `dim_amount` from `0.0` to `1.0`
- `tint = { color = "…", alpha = 0.0 }`

`fg`, `bg`, and `underline_color` accept solid colors and colors with alpha. Palette fields such as
`status.error` accept solid colors only.

### Color values

Solid colors can be written as:

- hex, such as `"#82aaff"`
- ANSI names, such as `"cyan"` or `"darkgray"`
- `indexed(0)` through `indexed(255)`
- `rgb(r,g,b)` with channels from 0 to 255

Style color fields also accept `"#RRGGBBAA"` and `rgba(r,g,b,a)`. Alpha is an integer from 0 to 255
or a decimal from `0.0` to `1.0`.

### Use the host background in one theme

Set `surface.backdrop = "backdrop"` to use the host terminal's background in this theme only:

```toml
extends = "nord"

[surface]
backdrop = "backdrop"
```

If the host background is not available, rozi uses the theme's panel color.

## Terminal colors

When you change themes, rozi applies the new theme's terminal palette to existing panes as well as
new ones. `ansi` and `system` pass the host terminal's palette through more directly. In a custom
theme, the status, accent, and related palette colors also change the pane's ANSI colors and pane
alerts.

## Command output colors

When you run rozi's own commands inside a rozi pane, their output follows the theme of the client
that shows the pane. This covers `rozi --help`, the tables printed by commands such as
`rozi list-panes`, and the progress rows of `rozi update` and extension installs.

- Headings, spinners, and the filled part of a download meter use the accent. The terminal
  palette's accent is slightly lighter than the interface accent.
- The first column of a table uses a dimmer accent.
- Muted text and the success, warning, and error colors use the theme's own colors.

Clients with different themes each see the same output in their own colors, and output already on
screen changes color when you switch themes.

Outside rozi, these commands use the default `rozi` palette, since the host terminal has its own
theme. The message printed when you detach from a session uses your active theme's accent and muted
colors. `NO_COLOR` and the other color switches turn styling off everywhere.

See [Terminal features](terminal.md) for clipboard, title, image, and scrollback behavior, which does
not depend on the theme.
