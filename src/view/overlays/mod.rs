use std::sync::Arc;

use tui_lipan::Justify::SpaceBetween;
use tui_lipan::prelude::*;
use tui_lipan::utils::color_contrast::readable_text_color;

use crate::input::Action;
use crate::state::{
    AppearanceAction, ProfilePickerState, ScrollbackMatch, ScrollbackSearchState,
    SessionPickerState,
};
use crate::{HyprmuxApp, Msg};

use super::keys::{
    appearance_palette_key, client_list_key, help_scroll_key, palette_key,
    pane_padding_horizontal_key, pane_padding_vertical_key, profile_picker_key, rename_input_key,
    rename_session_input_key, save_profile_key, search_input_key, session_picker_key,
    theme_picker_key,
};
use super::{
    action_palette_frame, action_palette_modal, action_palette_modal_with_width, fg_only,
    modal_scrollbar_config, shared_search_palette, styled_modal,
};

include!("help.rs");
include!("search.rs");
include!("prompts.rs");
include!("profiles.rs");
include!("sessions.rs");
include!("commands.rs");
include!("appearance.rs");
