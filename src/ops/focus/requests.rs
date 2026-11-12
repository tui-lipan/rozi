use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::pane::id::PaneId;
use crate::view;

pub(crate) fn request_pane_focus(ctx: &mut Context<AppRoot>, id: PaneId) {
    if ctx.state.has_modal_overlay() {
        return;
    }
    if let Some(pane) = crate::pane::lifecycle::find_displayed_pane(&ctx.state, id)
        .filter(|pane| pane.terminal_active && !pane.opening && !pane.closing)
    {
        let key = pane.keys.terminal.clone();
        focus_key(ctx, key);
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

pub(crate) fn request_recording_mark_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::recording_mark_input_key());
}

pub(crate) fn request_save_profile_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::save_profile_key());
}

pub(crate) fn request_profile_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::profile_picker_key());
}

pub(crate) fn request_worktree_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::worktree_picker_key());
}

pub(crate) fn request_worktree_form_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::worktree_form_input_key());
}

pub(crate) fn request_theme_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::theme_picker_key());
}

pub(crate) fn request_layout_picker_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::layout_picker_key());
}

pub(crate) fn request_welcome_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::welcome_key());
}

pub(crate) fn request_release_notes_focus(ctx: &mut Context<AppRoot>) {
    focus_key(ctx, view::release_notes_key());
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
    if ctx.state.askpass.is_some() || ctx.state.show_release_notes {
        return;
    }
    focus_key(ctx, view::session_picker_key());
}

pub(crate) fn request_remote_picker_focus(ctx: &mut Context<AppRoot>) {
    if ctx.state.askpass.is_some() || ctx.state.show_release_notes {
        return;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_from_a_palette_focuses_the_displayed_namespace_when_ids_collide() {
        crate::test_support::on_large_stack(|| {
            crate::test_support::isolate_user_dirs();
            let mut backend = tui_lipan::TestBackend::new(AppRoot::default());
            let (id, shared_key) = {
                let pane = &mut backend.state_mut().current_mut().workspaces[0].panes[0];
                pane.opening = false;
                pane.terminal_active = true;
                (pane.id, pane.keys.terminal.clone())
            };
            let mut scratch = crate::state::Pane::new_local(id, 100, FloatRect::default());
            scratch.opening = false;
            scratch.terminal_active = true;
            let local_key = scratch.keys.terminal.clone();
            backend.state_mut().scratch.panes.push(scratch);
            backend.state_mut().scratch.focused_pane = Some(id);
            for (visible, expected) in [(true, local_key), (false, shared_key)] {
                backend.state_mut().scratch_visible = visible;
                backend.state_mut().show_palette = true;
                backend.render();
                backend.advance(std::time::Duration::from_secs(1));
                backend.dispatch(crate::Msg::ClosePalette).unwrap();
                backend.render();
                assert_eq!(backend.focused_key(), Some(&expected));
            }
        });
    }
}
