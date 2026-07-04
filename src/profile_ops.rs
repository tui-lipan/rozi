use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{
    clear_default_profile, delete_profile_file, list_profiles, persist_default_profile,
    profile_path_for_name,
};
use crate::focus_ops::request_profile_picker_focus;
use crate::focus_ops::request_save_profile_focus;
use crate::pane_lifecycle;
use crate::profiles::{load_profile, profile_from_state, save_profile};
use crate::pty_events::{error_toast, info_toast};
use crate::startup_spawns;
use crate::state::{Mode, PaneRenameState, ProfilePickerState, State};
use crate::theme_ops;

pub(crate) fn open_save_profile_prompt(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.save_profile_prompt = Some(PaneRenameState::new(0, ""));
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    ctx.state.mode = Mode::Normal;
    request_save_profile_focus(ctx);
    Update::full()
}

pub(crate) fn close_save_profile_prompt(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.save_profile_prompt = None;
    Update::full()
}

pub(crate) fn submit_save_profile(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(name) = ctx
        .state
        .save_profile_prompt
        .as_ref()
        .and_then(|prompt| normalize_profile_name(prompt.input.text()))
    else {
        ctx.state.save_profile_prompt = None;
        return Update::full();
    };

    let path = profile_path_for_name(&name);
    let profile = profile_from_state(&ctx.state);
    match save_profile(&path, &profile) {
        Ok(()) => {
            ctx.toast().push(info_toast(format!(
                "Saved profile `{name}` to {}",
                path.display()
            )));
        }
        Err(message) => {
            ctx.toast().push(error_toast("Save Profile", message));
        }
    }

    ctx.state.save_profile_prompt = None;
    Update::full()
}

fn normalize_profile_name(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return None;
    }
    if name.contains(['/', '\\']) {
        return None;
    }
    Some(name.to_string())
}

pub(crate) fn open_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    let entries = list_profiles();
    ctx.state.profile_picker = Some(ProfilePickerState::new(entries));
    ctx.state.show_profile_picker = true;
    ctx.state.show_palette = false;
    ctx.state.show_help = false;
    ctx.state.search = None;
    ctx.state.rename = None;
    ctx.state.save_profile_prompt = None;
    ctx.state.mode = Mode::Normal;
    request_profile_picker_focus(ctx);
    Update::full()
}

pub(crate) fn cancel_profile_picker(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    Update::full()
}

pub(crate) fn profile_picker_query_changed(ctx: &mut Context<HyprmuxApp>, query: String) -> Update {
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        let cursor = query.len();
        picker.input.set_text(query);
        picker.input.set_cursor(cursor);
        picker.input.set_anchor(None);
        picker.selected = 0;
        picker.pending_delete = None;
    }
    request_profile_picker_focus(ctx);
    Update::full()
}

pub(crate) fn profile_picker_selection_changed(
    ctx: &mut Context<HyprmuxApp>,
    index: usize,
) -> Update {
    if let Some(picker) = ctx.state.profile_picker.as_mut() {
        picker.selected = index;
        if picker
            .pending_delete
            .is_some_and(|pending| pending != index)
        {
            picker.pending_delete = None;
        }
    }
    Update::full()
}

pub(crate) fn profile_picker_set_default(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };

    match persist_default_profile(&entry.name) {
        Ok(path) => {
            ctx.state.config.profile.default = Some(entry.name.clone());
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_delete = None;
            }
            ctx.toast().push(info_toast(format!(
                "Default profile set to `{}` in {}",
                entry.name,
                path.display()
            )));
        }
        Err(message) => {
            ctx.toast()
                .push(error_toast("Set Default Profile", message));
        }
    }
    Update::full()
}

pub(crate) fn profile_picker_delete_key(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(entry) = selected_profile_entry(ctx) else {
        return Update::none();
    };
    let index = ctx
        .state
        .profile_picker
        .as_ref()
        .map(|picker| picker.selected);

    let Some(index) = index else {
        return Update::none();
    };

    let confirm = ctx
        .state
        .profile_picker
        .as_ref()
        .is_some_and(|picker| picker.pending_delete == Some(index));

    if !confirm {
        if let Some(picker) = ctx.state.profile_picker.as_mut() {
            picker.pending_delete = Some(index);
        }
        return Update::full();
    }

    let name = entry.name.clone();
    let path = entry.path.clone();
    match delete_profile_file(&path) {
        Ok(()) => {
            if ctx.state.config.profile.default.as_deref() == Some(name.as_str()) {
                match clear_default_profile(&name) {
                    Ok(Some(config_path)) => {
                        ctx.state.config.profile.default = None;
                        ctx.toast().push(info_toast(format!(
                            "Cleared startup default in {}",
                            config_path.display()
                        )));
                    }
                    Ok(None) => {}
                    Err(message) => {
                        ctx.toast()
                            .push(error_toast("Clear Default Profile", message));
                    }
                }
            }
            refresh_profile_picker_entries(ctx);
            ctx.toast()
                .push(info_toast(format!("Deleted profile `{name}`")));
        }
        Err(message) => {
            ctx.toast().push(error_toast("Delete Profile", message));
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                picker.pending_delete = None;
            }
        }
    }
    Update::full()
}

pub(crate) fn select_profile(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some(entry) = ctx
        .state
        .profile_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(index).cloned())
    else {
        return Update::none();
    };

    let profile = match load_profile(&entry.path) {
        Ok(profile) => profile,
        Err(message) => {
            ctx.toast().push(error_toast("Load Profile", message));
            ctx.state.show_profile_picker = false;
            ctx.state.profile_picker = None;
            return Update::full();
        }
    };

    kill_all_live_ptys(&mut ctx.state);
    let theme_watcher = ctx.state.theme_watcher.take();
    let system_theme = ctx.state.system_theme.clone();
    let control_socket_path = ctx.state.control_socket_path.clone();
    let next_pty_generation = ctx.state.next_pty_generation;
    let runtime_epoch = ctx.state.runtime_epoch.saturating_add(1);
    let config = ctx.state.config.clone();
    let theme = ctx.state.theme.clone();

    let mut new_state = State::from_profile(config, theme, profile);
    new_state.theme_watcher = theme_watcher;
    new_state.system_theme = system_theme;
    new_state.control_socket_path = control_socket_path;
    new_state.next_pty_generation = next_pty_generation;
    new_state.runtime_epoch = runtime_epoch;
    ctx.state = new_state;
    theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
    ctx.toast()
        .push(info_toast(format!("Loaded profile `{}`", entry.name)));
    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    // The theme-tick and bar-tick loops started at app launch are
    // self-sustaining and survive the state swap, so don't restart them
    // here - doing so would spawn duplicate loops on every load.
    Update::with_command(pane_lifecycle::initial_command(
        startup_spawns(&mut ctx.state),
        false,
        false,
        None,
    ))
}

fn selected_profile_entry(ctx: &Context<HyprmuxApp>) -> Option<crate::config::ProfileEntry> {
    ctx.state
        .profile_picker
        .as_ref()
        .and_then(|picker| picker.entries.get(picker.selected).cloned())
}

fn refresh_profile_picker_entries(ctx: &mut Context<HyprmuxApp>) {
    let Some(picker) = ctx.state.profile_picker.as_mut() else {
        return;
    };
    picker.entries = list_profiles();
    picker.pending_delete = None;
    if picker.entries.is_empty() {
        picker.selected = 0;
        return;
    }
    picker.selected = picker.selected.min(picker.entries.len() - 1);
}

fn kill_all_live_ptys(state: &mut State) {
    for workspace in &mut state.workspaces {
        for pane in &mut workspace.panes {
            if !pane.closing {
                pane.closing = true;
                pane.terminal.kill();
            }
        }
    }
    if let Some(scratch) = state.scratch.as_mut() {
        scratch.closing = true;
        scratch.terminal.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Pane;

    fn rect() -> FloatRect {
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        }
    }

    #[test]
    fn profile_teardown_marks_panes_closing_before_kill() {
        let mut state = State::new(crate::config::HyprmuxConfig::default(), Theme::default());
        state.workspaces[0].panes.push(Pane::new(2, 100, rect()));
        state.scratch = Some(Pane::new(crate::state::SCRATCH_PANE_ID, 100, rect()));

        kill_all_live_ptys(&mut state);

        assert!(state.workspaces[0].panes.iter().all(|pane| pane.closing));
        assert!(state.scratch.as_ref().is_some_and(|pane| pane.closing));
    }
}
