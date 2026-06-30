use crate::state::PaneId;

pub fn pane_window_key(id: PaneId) -> String {
    format!("hyprmux-pane-{id}")
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

pub fn theme_picker_key() -> &'static str {
    "hyprmux-theme-picker"
}

pub fn palette_key() -> &'static str {
    "hyprmux-command-palette"
}

pub fn help_scroll_key() -> &'static str {
    "hyprmux-help-scroll"
}
