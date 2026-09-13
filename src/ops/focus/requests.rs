use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::PaneId;
use crate::view;

pub(crate) fn request_pane_focus(ctx: &mut Context<AppRoot>, id: PaneId) {
    if crate::pane::lifecycle::find_pane_mut(&mut ctx.state, id)
        .is_some_and(|pane| pane.terminal_active && !pane.opening && !pane.closing)
    {
        focus_key(ctx, view::pane_terminal_key(id));
    }
}

pub(crate) fn request_current_pane_focus(ctx: &mut Context<AppRoot>) {
    if let Some(id) = ctx.state.focused_pane() {
        request_pane_focus(ctx, id);
    }
}

/// Every "give focus to something that is not the sidebar" goes through here.
///
/// `sidebar.focused` records command-entered sidebar modality rather than framework focus alone,
/// so this is the one place that retracts it when an explicit request targets another region.
fn focus_key(ctx: &mut Context<AppRoot>, key: impl Into<tui_lipan::Key>) {
    ctx.state.sidebar.focused = false;
    ctx.request_focus(key);
}

pub(crate) fn request_search_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::search_input_key());
}

pub(crate) fn request_rename_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::rename_input_key());
}

pub(crate) fn request_rename_session_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::rename_session_input_key());
}

pub(crate) fn request_save_profile_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::save_profile_key());
}

pub(crate) fn request_profile_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::profile_picker_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::theme_picker_key());
}

pub(crate) fn request_layout_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::layout_picker_key());
}

pub(crate) fn request_extensions_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::extensions_key());
}

pub(crate) fn request_extension_detail_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::extension_detail_key());
}

pub(crate) fn request_extension_install_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::extension_install_input_key());
}

pub(crate) fn request_palette_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::palette_key());
}

pub(crate) fn request_session_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::session_picker_key());
}

pub(crate) fn request_remote_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::remote_picker_key());
}

pub(crate) fn request_agent_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::agent_picker_key());
}

/// Focus the host editor's active line.
///
/// Called when the form opens and when a submission is rejected — not on every keystroke. Between
/// those, `Tab`/`Shift+Tab` traversal owns focus and the form follows it; requesting focus while
/// the runtime is moving it is what sends the next keystroke to the line being left.
pub(crate) fn request_host_form_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::host_form_input_key());
}

/// Focus the ssh prompt modal. It is raised by a background thread rather than by a keypress, so
/// nothing else is moving focus onto it — and it has to take focus, or the answer would be typed
/// into whatever was focused when ssh asked.
///
/// A prompt answered by choosing has no field to focus; its answer row carries the cursor instead,
/// and opens on the affirmative.
pub(crate) fn request_askpass_focus(ctx: &mut Context<AppRoot>) {
    let choice = ctx
        .state
        .askpass
        .as_ref()
        .is_some_and(|askpass| askpass.current.kind.is_choice());
    if choice {
        request_dialog_answer_focus(ctx, view::DIALOG_AFFIRM);
    } else {
        focus_key(ctx, view::askpass_input_key());
    }
}

/// Focus one chip of a dialog's answer row.
pub(crate) fn request_dialog_answer_focus(ctx: &mut Context<AppRoot>, index: usize) {
    focus_key(ctx, view::dialog_answer_key(index));
}

pub(crate) fn request_pick_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::pick_key());
}

/// Focus the text prompt an action raised over the picker. The picker stays mounted underneath,
/// so focus has to move explicitly rather than being inherited.
pub(crate) fn request_pick_prompt_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::pick_prompt_input_key());
}
