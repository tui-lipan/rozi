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
visible = true
position = "right"
width = 34
tab_style = "round"'

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
