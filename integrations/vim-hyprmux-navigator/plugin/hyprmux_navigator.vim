if exists('g:loaded_hyprmux_navigator') || &compatible || v:version < 800
  finish
endif
let g:loaded_hyprmux_navigator = 1

let s:left_editor = 0

function! s:SaveBuffers() abort
  let l:mode = get(g:, 'hyprmux_navigator_save_on_switch', 0)
  if l:mode == 1
    silent! update
  elseif l:mode == 2
    silent! wall
  endif
endfunction

function! s:HyprmuxFocus(action) abort
  let l:command = get(g:, 'hyprmux_navigator_command', 'hyprmux')
  if empty($HYPRMUX_SOCKET) || !executable(l:command)
    return
  endif

  let l:action = a:action
  if !get(g:, 'hyprmux_navigator_wrap', 0) && l:action =~# '^focus-'
    let l:action .= '-no-wrap'
  endif

  call s:SaveBuffers()
  silent call system([l:command, 'run-action', l:action])
  let s:left_editor = v:shell_error == 0
endfunction

function! s:Check() abort
  let l:command = get(g:, 'hyprmux_navigator_command', 'hyprmux')
  echo 'command: ' . l:command
  echo 'executable: ' . executable(l:command)
  echo 'HYPRMUX: ' . $HYPRMUX
  echo 'HYPRMUX_PANE: ' . $HYPRMUX_PANE
  echo 'HYPRMUX_SOCKET: ' . $HYPRMUX_SOCKET
  echo 'Ctrl-h: ' . maparg('<C-h>', 'n')

  if empty($HYPRMUX_SOCKET) || !executable(l:command)
    return
  endif

  let l:output = system([l:command, 'run-action', 'focus-left'])
  echo 'focus-left exit: ' . v:shell_error
  echo 'focus-left output: ' . substitute(l:output, '\n\+$', '', '')
endfunction

function! s:Navigate(direction, action) abort
  let l:window = win_getid()
  try
    execute 'wincmd ' . a:direction
  catch /^Vim\%((\a\+)\)\=:E11/
    return
  endtry

  if win_getid() == l:window
    call s:HyprmuxFocus(a:action)
  else
    let s:left_editor = 0
  endif
endfunction

function! s:NavigatePrevious() abort
  if s:left_editor
    call s:HyprmuxFocus('cycle-focus-prev')
    return
  endif

  let l:window = win_getid()
  silent! wincmd p
  if win_getid() == l:window
    call s:HyprmuxFocus('cycle-focus-prev')
  endif
endfunction

command! HyprmuxNavigateLeft call <SID>Navigate('h', 'focus-left')
command! HyprmuxNavigateDown call <SID>Navigate('j', 'focus-down')
command! HyprmuxNavigateUp call <SID>Navigate('k', 'focus-up')
command! HyprmuxNavigateRight call <SID>Navigate('l', 'focus-right')
command! HyprmuxNavigatePrevious call <SID>NavigatePrevious()
command! HyprmuxNavigatorCheck call <SID>Check()

if !get(g:, 'hyprmux_navigator_no_mappings', 0)
  nnoremap <silent> <C-h> :<C-U>HyprmuxNavigateLeft<CR>
  nnoremap <silent> <C-j> :<C-U>HyprmuxNavigateDown<CR>
  nnoremap <silent> <C-k> :<C-U>HyprmuxNavigateUp<CR>
  nnoremap <silent> <C-l> :<C-U>HyprmuxNavigateRight<CR>
  nnoremap <silent> <C-\> :<C-U>HyprmuxNavigatePrevious<CR>

  if !empty($HYPRMUX)
    tnoremap <silent> <C-h> <C-w>:HyprmuxNavigateLeft<CR>
    tnoremap <silent> <C-j> <C-w>:HyprmuxNavigateDown<CR>
    tnoremap <silent> <C-k> <C-w>:HyprmuxNavigateUp<CR>
    tnoremap <silent> <C-l> <C-w>:HyprmuxNavigateRight<CR>
  endif
endif
