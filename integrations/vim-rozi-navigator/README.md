# vim-rozi-navigator

Seamless navigation between Vim/Neovim splits and rozi panes.

## Setup

Enable rozi's split-aware bindings in `~/.config/rozi/config.toml`:

```toml
[keys]
smart-focus-left = "ctrl-h"
smart-focus-down = "ctrl-j"
smart-focus-up = "ctrl-k"
smart-focus-right = "ctrl-l"
```

### Register the Rozi extension

Link or copy this repository into Rozi's extension directory:

```bash
data_root="${XDG_DATA_HOME:-$HOME/.local/share}/rozi"
install -d -m 700 "$data_root" "$data_root/extensions"
ln -s "/absolute/path/to/rozi/integrations/vim-rozi-navigator" \
  "$data_root/extensions/vim-rozi-navigator"
rozi extensions check "$data_root/extensions/vim-rozi-navigator"
rozi run-action reload-extensions
```

The manifest contributes the Vim-family foreground program names. Rozi currently ships the same
common names as compatibility defaults, so identical registrations are deduplicated. If
`[navigation] editors` appears in `config.toml`, that explicit list replaces both defaults and
extension targets.

### Install the Vim or Neovim plugin

Registering the Rozi extension does not install the editor plugin. Install this same repository
through Vim's package mechanism. For example, from a Rozi checkout:

```bash
ln -s "$PWD/integrations/vim-rozi-navigator" \
  ~/.vim/pack/plugins/start/vim-rozi-navigator
```

For Neovim with lazy.nvim:

```lua
{
  dir = "/path/to/rozi/integrations/vim-rozi-navigator",
  name = "vim-rozi-navigator",
  keys = {
    { "<C-h>", "<cmd>RoziNavigateLeft<cr>", mode = "n" },
    { "<C-j>", "<cmd>RoziNavigateDown<cr>", mode = "n" },
    { "<C-k>", "<cmd>RoziNavigateUp<cr>", mode = "n" },
    { "<C-l>", "<cmd>RoziNavigateRight<cr>", mode = "n" },
    { "<C-\\>", "<cmd>RoziNavigatePrevious<cr>", mode = "n" },
  },
}
```

Declaring `keys` is important with LazyVim because its default window-navigation mappings would
otherwise replace the plugin's mappings later during startup.

The defaults are `Ctrl-h/j/k/l` for left/down/up/right and `Ctrl-\` for the previous split or
pane. Directional mappings also work in Vim/Neovim terminal mode.

The repository contains two independently installed parts. `extension.toml` teaches Rozi which
foreground programs are split-aware; `plugin/` and `doc/` teach Vim how to cross an outer split
edge. Rozi compiles the static manifest declaration when extensions load, while the editor plugin
crosses the edge through Rozi's public `run-action` CLI. No extension process or callback runs in
the key-input path.

## Configuration

Set options before the plugin loads:

```vim
" Use an absolute path when rozi is not installed on PATH.
let g:rozi_navigator_command = "/path/to/rozi"

" Define commands without installing the default mappings.
let g:rozi_navigator_no_mappings = 1

" 1 runs :update; 2 runs :wall before focus leaves the editor.
let g:rozi_navigator_save_on_switch = 2

" Wrap to another rozi pane at the outer edge (disabled by default).
let g:rozi_navigator_wrap = 1
```

Custom mappings can call `:RoziNavigateLeft`, `Down`, `Up`, `Right`, or `Previous`.

The plugin first uses `:wincmd` and invokes `rozi run-action` only at an editor split edge. By
default, focus stays put when no rozi pane exists in that direction. Set
`g:rozi_navigator_wrap = 1` to retain rozi's normal edge wrapping. If Vim is not running
inside rozi, the commands continue to work as ordinary split navigation.

Run `:RoziNavigatorCheck` inside rozi to print the active mapping and environment and send a
test `focus-left` request through the control socket.
