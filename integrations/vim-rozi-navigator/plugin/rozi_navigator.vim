if exists('g:loaded_rozi_navigator') || &compatible || v:version < 800
  finish
endif
let g:loaded_rozi_navigator = 1

let s:left_editor = 0

function! s:SaveBuffers() abort
  let l:mode = get(g:, 'rozi_navigator_save_on_switch', 0)
  if l:mode == 1
    silent! update
  elseif l:mode == 2
    silent! wall
  endif
endfunction

function! s:RoziFocus(action) abort
  let l:command = get(g:, 'rozi_navigator_command', 'rozi')
  if empty($ROZI_SOCKET) || !executable(l:command)
    return
  endif

  let l:action = a:action
  if !get(g:, 'rozi_navigator_wrap', 0) && l:action =~# '^focus-'
    let l:action .= '-no-wrap'
  endif

  call s:SaveBuffers()
  silent call system([l:command, 'run-action', l:action])
  let s:left_editor = v:shell_error == 0
endfunction

function! s:Check() abort
  let l:command = get(g:, 'rozi_navigator_command', 'rozi')
  echo 'command: ' . l:command
  echo 'executable: ' . executable(l:command)
  echo 'ROZI: ' . $ROZI
  echo 'ROZI_PANE: ' . $ROZI_PANE
  echo 'ROZI_SOCKET: ' . $ROZI_SOCKET
  echo 'Ctrl-h: ' . maparg('<C-h>', 'n')

  if empty($ROZI_SOCKET) || !executable(l:command)
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
    call s:RoziFocus(a:action)
  else
    let s:left_editor = 0
  endif
endfunction

function! s:NavigatePrevious() abort
  if s:left_editor
    call s:RoziFocus('cycle-focus-prev')
    return
  endif

  let l:window = win_getid()
  silent! wincmd p
  if win_getid() == l:window
    call s:RoziFocus('cycle-focus-prev')
  endif
endfunction

command! RoziNavigateLeft call <SID>Navigate('h', 'focus-left')
command! RoziNavigateDown call <SID>Navigate('j', 'focus-down')
command! RoziNavigateUp call <SID>Navigate('k', 'focus-up')
command! RoziNavigateRight call <SID>Navigate('l', 'focus-right')
command! RoziNavigatePrevious call <SID>NavigatePrevious()
command! RoziNavigatorCheck call <SID>Check()

if !get(g:, 'rozi_navigator_no_mappings', 0)
  nnoremap <silent> <C-h> :<C-U>RoziNavigateLeft<CR>
  nnoremap <silent> <C-j> :<C-U>RoziNavigateDown<CR>
  nnoremap <silent> <C-k> :<C-U>RoziNavigateUp<CR>
  nnoremap <silent> <C-l> :<C-U>RoziNavigateRight<CR>
  nnoremap <silent> <C-\> :<C-U>RoziNavigatePrevious<CR>

  if !empty($ROZI)
    tnoremap <silent> <C-h> <C-w>:RoziNavigateLeft<CR>
    tnoremap <silent> <C-j> <C-w>:RoziNavigateDown<CR>
    tnoremap <silent> <C-k> <C-w>:RoziNavigateUp<CR>
    tnoremap <silent> <C-l> <C-w>:RoziNavigateRight<CR>
  endif
endif
