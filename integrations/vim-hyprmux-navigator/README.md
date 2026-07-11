# vim-hyprmux-navigator

Seamless navigation between Vim/Neovim splits and hyprmux panes.

## Setup

Enable hyprmux's editor-aware bindings in `~/.config/hyprmux/hyprmux.toml`:

```toml
[keys]
smart-focus-left = "ctrl-h"
smart-focus-down = "ctrl-j"
smart-focus-up = "ctrl-k"
smart-focus-right = "ctrl-l"
```

Install this directory as a Vim package. For example, from a hyprmux checkout:

```bash
ln -s "$PWD/integrations/vim-hyprmux-navigator" \
  ~/.vim/pack/plugins/start/vim-hyprmux-navigator
```

For Neovim with lazy.nvim:

```lua
{
  dir = "/path/to/hyprmux/integrations/vim-hyprmux-navigator",
  name = "vim-hyprmux-navigator",
  keys = {
    { "<C-h>", "<cmd>HyprmuxNavigateLeft<cr>", mode = "n" },
    { "<C-j>", "<cmd>HyprmuxNavigateDown<cr>", mode = "n" },
    { "<C-k>", "<cmd>HyprmuxNavigateUp<cr>", mode = "n" },
    { "<C-l>", "<cmd>HyprmuxNavigateRight<cr>", mode = "n" },
    { "<C-\\>", "<cmd>HyprmuxNavigatePrevious<cr>", mode = "n" },
  },
}
```

Declaring `keys` is important with LazyVim because its default window-navigation mappings would
otherwise replace the plugin's mappings later during startup.

The defaults are `Ctrl-h/j/k/l` for left/down/up/right and `Ctrl-\` for the previous split or
pane. Directional mappings also work in Vim/Neovim terminal mode.

## Configuration

Set options before the plugin loads:

```vim
" Use an absolute path when hyprmux is not installed on PATH.
let g:hyprmux_navigator_command = "/path/to/hyprmux"

" Define commands without installing the default mappings.
let g:hyprmux_navigator_no_mappings = 1

" 1 runs :update; 2 runs :wall before focus leaves the editor.
let g:hyprmux_navigator_save_on_switch = 2

" Wrap to another hyprmux pane at the outer edge (disabled by default).
let g:hyprmux_navigator_wrap = 1
```

Custom mappings can call `:HyprmuxNavigateLeft`, `Down`, `Up`, `Right`, or `Previous`.

The plugin first uses `:wincmd` and invokes `hyprmux run-action` only at an editor split edge. By
default, focus stays put when no hyprmux pane exists in that direction. Set
`g:hyprmux_navigator_wrap = 1` to retain hyprmux's normal edge wrapping. If Vim is not running
inside hyprmux, the commands continue to work as ordinary split navigation.

Run `:HyprmuxNavigatorCheck` inside hyprmux to print the active mapping and environment and send a
test `focus-left` request through the control socket.
