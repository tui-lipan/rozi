use crate::state::PaneId;

pub fn pane_window_key(id: PaneId, generation: u64) -> String {
    format!("hyprmux-pane-{id}-{generation}")
}

pub fn pane_body_key(id: PaneId) -> String {
    format!("hyprmux-pane-body-{id}")
}

pub fn pane_terminal_key(id: PaneId) -> String {
    format!("hyprmux-terminal-{id}")
}

pub fn search_input_key() -> &'static str {
    "hyprmux-search-input"
}

pub fn rename_input_key() -> &'static str {
    "hyprmux-rename-input"
}

pub fn rename_session_input_key() -> &'static str {
    "hyprmux-rename-session-input"
}

pub fn save_profile_key() -> &'static str {
    "hyprmux-save-profile-input"
}

pub fn profile_picker_key() -> &'static str {
    "hyprmux-profile-picker"
}

pub fn session_picker_key() -> &'static str {
    "hyprmux-session-picker"
}

pub fn collaboration_key() -> &'static str {
    "hyprmux-collaboration"
}

pub fn follow_prompt_key() -> &'static str {
    "hyprmux-follow-prompt"
}

pub fn theme_picker_key() -> &'static str {
    "hyprmux-theme-picker"
}

pub fn layout_picker_key() -> &'static str {
    "hyprmux-layout-picker"
}

pub fn palette_key() -> &'static str {
    "hyprmux-command-palette"
}

/// The sidebar's row list — the `request_focus` target for every tab except the file tree, which
/// mounts under its own root-derived key.
pub fn sidebar_body_key() -> &'static str {
    "hyprmux-sidebar-body"
}

pub fn appearance_palette_key() -> &'static str {
    "hyprmux-appearance-palette"
}

pub fn pane_padding_vertical_key() -> &'static str {
    "hyprmux-pane-padding-vertical"
}
pub fn pane_padding_horizontal_key() -> &'static str {
    "hyprmux-pane-padding-horizontal"
}

pub fn help_scroll_key() -> &'static str {
    "hyprmux-help-scroll"
}
