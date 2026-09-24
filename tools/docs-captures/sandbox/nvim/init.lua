vim.o.termguicolors = false
vim.o.number = true
vim.o.cursorline = true
vim.o.signcolumn = "yes"
vim.o.laststatus = 2
vim.o.showmode = false
vim.o.shortmess = vim.o.shortmess .. "I"
vim.cmd("syntax on")
vim.api.nvim_set_hl(0, "Normal", { ctermbg = "NONE" })
vim.api.nvim_set_hl(0, "NormalNC", { ctermbg = "NONE" })
vim.api.nvim_set_hl(0, "SignColumn", { ctermbg = "NONE" })
vim.api.nvim_set_hl(0, "LineNr", { ctermfg = 8 })
vim.api.nvim_set_hl(0, "CursorLineNr", { ctermfg = 5, bold = true })
vim.api.nvim_set_hl(0, "CursorLine", { ctermbg = "NONE", underline = false })
vim.api.nvim_set_hl(0, "StatusLine", { ctermfg = 0, ctermbg = 4, bold = true })
vim.api.nvim_set_hl(0, "StatusLineNC", { ctermfg = 8, ctermbg = "NONE" })
vim.o.wrap = false
