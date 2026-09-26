pub(in crate::view::overlays) use std::str::FromStr;
pub(in crate::view::overlays) use std::sync::Arc;

pub(in crate::view::overlays) use tui_lipan::prelude::*;
pub(in crate::view::overlays) use tui_lipan::rank_search_palette_indices_with_mode;
pub(in crate::view::overlays) use tui_lipan::utils::color_contrast::readable_text_color;

pub(in crate::view::overlays) use crate::input::Action;
pub(in crate::view::overlays) use crate::state::{
    MAX_MATCHES, ProfilePickerState, RemoteSessionIdentity, ScrollbackSearchState,
    SessionPickerState, SettingsAction, cap_style_label,
};
pub(in crate::view::overlays) use crate::{AppRoot, Msg};

pub(in crate::view::overlays) use super::widget_keys::{
    askpass_input_key, collaboration_key, extension_detail_key, extension_install_error_key,
    extension_install_input_key, extensions_key, help_filter_key, keybinding_capture_key,
    layout_picker_key, palette_key, pane_padding_horizontal_key, pane_padding_vertical_key,
    pick_key, pick_prompt_input_key, profile_picker_key, recording_mark_input_key,
    remote_picker_key, rename_input_key, rename_session_input_key, save_profile_key,
    search_input_key, session_picker_key, settings_choice_key, settings_palette_key,
    theme_picker_key,
};
pub(in crate::view::overlays) use super::{
    ACTION_PALETTE_MAX_HEIGHT_PERCENT, action_palette_modal, action_palette_modal_with_width,
    fg_only, modal_scrollbar_config, nested_action_palette_modal, overlay_border_style,
    rozi_chrome, rozi_fg, search_entries_with_groups, shared_search_palette, styled_modal,
};

mod agents;
mod commands;
mod common;
mod confirm;
mod extensions;
mod help;
mod layout;
mod palette;
mod pick;
mod profiles;
mod prompts;
mod remotes;
mod search;
mod sessions;
mod settings;
mod worktrees;

pub(crate) use agents::agent_picker_overlay;
pub(crate) use commands::palette_overlay;
pub(crate) use confirm::{DIALOG_AFFIRM, DIALOG_REFUSE};
pub(crate) use extensions::{
    extension_detail_overlay, extension_install_progress_overlay, extensions_overlay,
};
pub(crate) use help::{help_overlay, keybinding_editor_dialog_overlay, neighbor_keybinding_id};
pub(crate) use layout::layout_picker_overlay;
pub(crate) use pick::{pick_overlay, pick_prompt_overlay};
pub(crate) use profiles::profile_picker_overlay;
pub(crate) use prompts::{
    askpass_overlay, extension_install_prompt_overlay, recording_mark_overlay, rename_overlay,
    rename_session_overlay, save_profile_overlay,
};
pub(crate) use remotes::remote_picker_overlay;
pub(crate) use search::search_overlay;
pub(crate) use sessions::{
    collaboration_overlay, follow_prompt_overlay, reconnecting_overlay, session_picker_overlay,
};
pub(crate) use settings::{
    pane_padding_overlay, settings_choice_overlay, settings_overlay, settings_query_selection,
    theme_picker_overlay,
};
pub(crate) use worktrees::worktree_overlay;

pub(in crate::view::overlays) use commands::settings_palette_aliases;
pub(in crate::view::overlays) use common::{ctrl_letter, hint_pill, hint_row};
pub(in crate::view::overlays) use confirm::{DialogButton, DialogChrome, dialog_overlay};
pub(in crate::view::overlays) use palette::{
    OverlayAction, OverlayItemRenderer, OverlayPalette, OverlayTabs, overlay_hints,
    overlay_interceptor, picker_description, picker_row, picker_selection_style,
};
pub(in crate::view::overlays) use pick::fit_description;
pub(in crate::view::overlays) use profiles::render_ephemeral_session_item;
pub(in crate::view::overlays) use prompts::{
    BACKDROP_RECESSION, PromptCaption, PromptChrome, action_palette, prompt_caption_accent,
    prompt_detail_row, prompt_highlight_row, prompt_overlay,
};
pub(in crate::view::overlays) use sessions::session_description;
pub(in crate::view::overlays) use settings::action_search_palette;
