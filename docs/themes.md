# Themes

Open **Settings**, choose **Theme**, and select a theme. Rozi previews the highlighted theme.
Selecting it writes `[theme].name` to `config.toml`.

You can also set it directly:

```toml
[theme]
name = "tokyo-night"
```

The active theme controls Rozi's interface and the ANSI palette used by pane terminals.

## Choose a built-in theme

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

Theme ids are case-insensitive. Underscores and spaces normalize to hyphens. Common compact aliases
such as `onedark`, `tokyonight`, `gruvbox`, `catppuccin`, and `solarized` are accepted.

`ansi` is a valid config value but is not shown in the picker.

## Use the host terminal colors

Set `name = "system"` to derive the theme from the host terminal's foreground, background, and ANSI
colors. If the terminal cannot answer the color query, Rozi uses ANSI colors for that run and warns.
It keeps `system` configured and tries again next launch.

Use **Background follows terminal** in Settings, or:

```toml
[pane]
background_follows_terminal = true
```

This keeps the chosen theme but replaces its backdrop with the host terminal's reported background.
Rozi queries the color at startup. Restart after changing the host terminal theme.

## Create a custom theme

Place a TOML file in `~/.config/rozi/themes/`. The filename is the theme id:

```toml
# ~/.config/rozi/themes/my-nord.toml
extends = "nord"

[accent]
fg = "#ff79c6"

[status]
success = "#50fa7b"
error = "#ff5555"
```

Then select `my-nord`. A custom file shadows a built-in theme or the reserved `system` name with the
same id.

`extends` is optional. Without it, the file starts from `lipan`. It accepts every preset in the
table except Rozi's app-specific `rozi` theme, plus `ansi` and `lipan`. It does not accept
`system`. Matching ignores case, hyphens, underscores, and spaces.

Rozi watches the active custom file and reloads it after changes. Parse errors produce a warning and
use `lipan` until the file is fixed. Built-in and system themes do not have a file to reload.

## Theme file reference

A custom theme is a partial overlay. Omitted fields keep their value from `extends`.

### Top-level fields

The style tables are `primary`, `accent`, `selection`, `text_selection`, `focus`, `hover`, `border`,
and `muted`.

Other top-level fields are:

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
- `tint = { color = "...", alpha = 0.0 }`

`fg`, `bg`, and `underline_color` accept solid colors and alpha paint. Palette fields such as
`status.error` accept solid colors.

### Color values

Solid colors accept:

- hex, such as `"#82aaff"`
- ANSI names, such as `"cyan"` or `"darkgray"`
- `indexed(0)` through `indexed(255)`
- `rgb(r,g,b)` with channels from 0 to 255

Style paint fields also accept `"#RRGGBBAA"` and `rgba(r,g,b,a)`. Alpha may be an integer from 0 to
255 or a decimal from `0.0` to `1.0`.

Set `surface.backdrop = "backdrop"` in one custom theme to use the host terminal background for
that theme only:

```toml
extends = "nord"

[surface]
backdrop = "backdrop"
```

If no host background is available, Rozi uses the theme's panel color.

## Terminal colors

Rozi applies the active theme's terminal palette to new and existing panes when a theme changes.
The `ansi` and `system` choices use the host terminal palette more directly. Custom status, accent,
and related palette colors also affect terminal ANSI colors and pane alerts.

See [Terminal features](terminal.md) for clipboard, title, image, and scrollback behavior that is
independent of the selected theme.
