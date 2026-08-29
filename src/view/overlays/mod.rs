use std::str::FromStr;
use std::sync::Arc;

use tui_lipan::Justify::SpaceBetween;
use tui_lipan::prelude::*;
use tui_lipan::rank_search_palette_indices_with_mode;
use tui_lipan::utils::color_contrast::readable_text_color;

use crate::input::Action;
use crate::state::{
    MAX_MATCHES, ProfilePickerState, RemoteSessionIdentity, ScrollbackSearchState,
    SessionPickerState, SettingsAction, cap_style_label,
};
use crate::{AppRoot, Msg};

use super::keys::{
    collaboration_key, help_filter_key, help_scroll_key, layout_picker_key, palette_key,
    pane_padding_horizontal_key, pane_padding_vertical_key, pick_key, pick_prompt_input_key,
    profile_picker_key, rename_input_key, rename_session_input_key, save_profile_key,
    remote_picker_key, search_input_key, session_picker_key, settings_palette_key,
    theme_picker_key,
};
use super::{
    action_palette_frame, action_palette_modal, action_palette_modal_with_width, fg_only,
    modal_scrollbar_config, search_entries_with_groups, shared_search_palette, styled_modal,
};

include!("search.rs");
include!("prompts.rs");
include!("profiles.rs");
include!("sessions.rs");
include!("remotes.rs");
include!("commands.rs");
include!("settings.rs");
include!("layout.rs");
include!("pick.rs");
// Last: help.rs ends in a `#[cfg(test)]` module, and `include!` splices these files into one
// module, so any include after it would put items behind that test module.
include!("help.rs");
