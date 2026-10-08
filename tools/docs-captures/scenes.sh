# Scenes for capture.sh. Each scene produces docs/assets/captures/<name>.webp.
# shellcheck shell=bash disable=SC2154

wide=160x45
focus=120x34

# The hero shot: an editor, Git, a system monitor, and a shell in one workspace.
scene workspace "$wide" 5000 "wait:10"

# One workspace, six themes. Programs in panes use ANSI colors, so each theme recolors them too.
for theme in catppuccin-mocha tokyo-night gruvbox-dark nord rose-pine-dawn; do
  scene "theme-$theme" "$wide" 5000 "wait:10" "[theme]
name = \"$theme\""
done

# Pane chrome styles.
scene style-merged "$wide" 5000 "wait:10" '[pane]
border_mode = "merged"
titlebar = "integrated"
title_style = "round"'
scene style-minimal "$wide" 5000 "wait:10" '[pane]
border_mode = "dividers"
titlebar = "inset"
padding = [0, 1]
workbar_background = false'
scene style-powerline "$wide" 5000 "wait:10" '[pane]
border_style = "thick"
title_style = "arrow"
workbar_style = "arrow"
workbar_tab_style = "arrow"
workbar_badge_style = "arrow"'

# The complete example from customize.md, minus its cwd and keys.
PROFILE=web LAYOUT=master scene personalized "$wide" 5000 "wait:10" '[theme]
name = "gruvbox-dark"

[pane]
border_mode = "merged"
border_style = "thick"
float_border_style = "double"
titlebar = "border"
title_style = "round"
padding = [0, 1]
workbar_at_bottom = true
workbar_style = "round"

[workbar]
left = ["workspaces"]
right = ["layout", "clock", "session"]
clock_format = "%H:%M"

[sidebar]
startup = "right"
layout = { right = { width = 34, panel_count = 2, panels = [{ weight = 0.4, tabs = ["activity", "panes", "sessions"] }, { weight = 0.6, tabs = ["files", "git", "worktrees"] }] } }
tab_style = "round"

[animations]
session = "off"'

# Layouts, from the same five panes.
for layout in dwindle master grid columns scrollable monocle; do
  PROFILE=web LAYOUT=$layout scene "layout-$layout" "$wide" 4500 "wait:10"
done

# The keybinding editor, as a four-step story.
open_keys="key:ctrl+a; type:?; wait:500; type:new pane; wait:400"
PROFILE=api scene keys-find "$focus" 4000 "$open_keys"
PROFILE=api scene keys-record "$focus" 4000 "$open_keys; key:enter; wait:400; key:ctrl+alt+n; wait:400"
PROFILE=api scene keys-conflict "$focus" 4000 "$open_keys; key:enter; wait:400; key:alt+w; wait:400"
PROFILE=api scene keys-saved "$focus" 4000 "$open_keys; key:enter; wait:400; key:ctrl+alt+n; wait:400; key:enter; wait:600"

# Overlays.
open_settings="key:ctrl+a; type:p; wait:500; type:settings; wait:300; key:enter; wait:600"
PROFILE=api scene settings "$focus" 4000 "$open_settings"
PROFILE=api scene settings-theme-preview "$focus" 4000 "$open_settings; key:enter; wait:500; type:tokyo; wait:800"
PROFILE=api scene which-key "$focus" 4000 "key:ctrl+a; wait:800"
PROFILE=api scene palette "$focus" 4000 "key:ctrl+a; type:p; wait:600"
PROFILE=web scene sidebar "$wide" 4500 "key:ctrl+a; type:b; wait:600; key:ctrl+a; key:pagedown; wait:600"

# Independent docks and visibility controls, using the same saved placement.
# Static captures skip the session portal; its paint-only progress needs a live terminal.
sidebar_docks='[sidebar]
startup = "both"
layout = { left = { width = 32, panel_count = 3, panels = [{ weight = 1.0, tabs = ["panes"] }, { weight = 1.0, tabs = ["sessions"] }, { weight = 1.0, tabs = ["activity"] }] }, right = { width = 36, panel_count = 3, panels = [{ weight = 1.0, tabs = ["files"] }, { weight = 1.0, tabs = ["git"] }, { weight = 1.0, tabs = ["worktrees"] }] } }
[animations]
session = "off"'
PROFILE=web scene sidebar-docks "$wide" 4500 "wait:600" "$sidebar_docks"
sidebar_visibility=${sidebar_docks/left =/hidden = [\"sessions\", \"git\", \"worktrees\"], left =}
PROFILE=web scene sidebar-manager "$focus" 4500 "key:ctrl+a; type:p; wait:300; type:sidebar tabs; key:enter; wait:500" "$sidebar_visibility"
PROFILE=web scene sidebar-manager-compact 80x24 4500 "key:ctrl+a; type:p; wait:300; type:sidebar tabs; key:enter; wait:500" "$sidebar_visibility"
PROFILE=web scene sidebar-settings "$focus" 4500 "$open_settings; type:sidebar; wait:400" "$sidebar_visibility"
PROFILE=web scene sidebar-panels "$focus" 4500 "$open_settings; type:left panels; key:enter; wait:400" "$sidebar_visibility"
PROFILE=web scene sidebar-startup "$focus" 4500 "$open_settings; type:sidebars at startup; key:enter; wait:400" "$sidebar_visibility"
PROFILE=web scene sidebar-commands "$focus" 4500 "key:ctrl+a; type:p; wait:300; type:sidebar; wait:400" "$sidebar_visibility"
PROFILE=web scene sidebar-keys "$focus" 4500 "key:ctrl+a; key:shift+b; type:?; wait:400" "$sidebar_visibility"
# Focus Right, return to the pane, hide Right, then focus the still-shown Left dock.
PROFILE=web scene sidebar-focus-visible "$wide" 4500 "key:ctrl+a; key:shift+b; wait:300; key:ctrl+right; wait:300; click:60,15; wait:300; key:ctrl+a; type:p; wait:300; type:right sidebar; key:enter; wait:300; key:ctrl+a; key:shift+b; wait:600" "$sidebar_visibility"
# Hide each dock separately, then restore only the one that was visible last.
PROFILE=web scene sidebar-restore "$wide" 4500 "key:ctrl+a; type:p; type:right sidebar; key:enter; wait:300; key:ctrl+a; type:p; type:left sidebar; key:enter; wait:300; key:ctrl+a; type:b; wait:600" "$sidebar_visibility"



# Recording indicators. Animations are off so the blinking dots hold steady; the long wait lets
# each start's toast expire.
run_palette() { echo "key:ctrl+a; type:p; wait:500; type:$1; wait:300; key:enter; sleep:1500"; }
steady='[animations]
enabled = false'
start_pane="key:alt+l; wait:300; $(run_palette 'start pane recording')"
start_ui=$(run_palette 'start ui recording')
PROFILE=api scene recording-pane "$focus" 4000 "$start_pane; wait:8000" "$steady"
PROFILE=api scene recording-ui "$focus" 4000 "$start_ui; wait:8000" "$steady"
PROFILE=api scene recording-fullscreen "$focus" 4000 "$start_pane; $start_ui; key:alt+f; wait:8000" "$steady"

# Inside a recording of the whole UI: a palette screenshot of the editor, an image in the shell,
# then a pane recording started, marked, and stopped from that shell.
PROFILE=api POSTER=13.8 clip record-demo "$focus" 5000 "
sleep:800
key:C-a
sleep:150
type:p
sleep:600
type:screenshot pane
sleep:500
key:Enter
sleep:2000
key:M-l
sleep:600
type:icat assets/logo.png 20 9
sleep:300
key:Enter
sleep:1200
type:rozi record start --output ~/demo.rozirec
sleep:300
key:Enter
sleep:1400
type:rozi record mark 'logo shown'
sleep:300
key:Enter
sleep:1400
type:rozi record stop
sleep:300
key:Enter
sleep:2000" '[capture]
dir = "~/shots"'

sessions_setup='
for s in api docs infra; do rozi --session "$s" --server >/dev/null 2>&1 & done
sleep 1.5
rozi --session api split >/dev/null 2>&1 || true
rozi --session api split >/dev/null 2>&1 || true
rozi --session docs split >/dev/null 2>&1 || true
rozi --session infra split >/dev/null 2>&1 || true
rozi --session infra split >/dev/null 2>&1 || true
sleep 0.5
'
STARTUP=picker SETUP=$sessions_setup scene session-picker "$focus" 3000 "wait:400"
