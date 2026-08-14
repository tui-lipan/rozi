use crate::state::PaneId;

pub fn pane_window_key(id: PaneId, generation: u64) -> String {
    format!("rozi-pane-{id}-{generation}")
}

pub fn pane_body_key(id: PaneId) -> String {
    format!("rozi-pane-body-{id}")
}

pub fn pane_terminal_key(id: PaneId) -> String {
    format!("rozi-terminal-{id}")
}

pub fn search_input_key() -> &'static str {
    "rozi-search-input"
}

pub fn rename_input_key() -> &'static str {
    "rozi-rename-input"
}

pub fn rename_session_input_key() -> &'static str {
    "rozi-rename-session-input"
}

pub fn save_profile_key() -> &'static str {
    "rozi-save-profile-input"
}

pub fn profile_picker_key() -> &'static str {
    "rozi-profile-picker"
}

pub fn session_picker_key() -> &'static str {
    "rozi-session-picker"
}

pub fn collaboration_key() -> &'static str {
    "rozi-collaboration"
}

pub fn follow_prompt_key() -> &'static str {
    "rozi-follow-prompt"
}

pub fn theme_picker_key() -> &'static str {
    "rozi-theme-picker"
}

pub fn layout_picker_key() -> &'static str {
    "rozi-layout-picker"
}

pub fn palette_key() -> &'static str {
    "rozi-command-palette"
}

pub fn pick_key() -> &'static str {
    "rozi-pick"
}

/// The text prompt an action raises over the picker.
pub fn pick_prompt_input_key() -> &'static str {
    "rozi-pick-prompt-input"
}

/// The sidebar's row list — the `request_focus` target for every tab except the file tree, which
/// mounts under its own root-derived key.
pub fn sidebar_body_key() -> &'static str {
    "rozi-sidebar-body"
}

pub fn settings_palette_key() -> &'static str {
    "rozi-settings-palette"
}

pub fn pane_padding_vertical_key() -> &'static str {
    "rozi-pane-padding-vertical"
}
pub fn pane_padding_horizontal_key() -> &'static str {
    "rozi-pane-padding-horizontal"
}

pub fn help_scroll_key() -> &'static str {
    "rozi-help-scroll"
}
