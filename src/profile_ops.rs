use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::config::{list_profiles, persist_default_profile, profile_path_for_name};
use crate::focus_ops::request_profile_picker_focus;
use crate::focus_ops::request_save_profile_focus;
use crate::pane_lifecycle;
use crate::profiles::{load_profile, profile_from_state, save_profile};
use crate::pty_events::{error_toast, info_toast};
use crate::startup_spawns;
use crate::state::{Mode, PaneRenameState, ProfilePickerMode, ProfilePickerState, State};
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

pub(crate) fn open_profile_picker(
    ctx: &mut Context<HyprmuxApp>,
    mode: ProfilePickerMode,
) -> Update {
    let entries = list_profiles();
    ctx.state.profile_picker = Some(ProfilePickerState::new(mode, entries));
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
    }
    request_profile_picker_focus(ctx);
    Update::full()
}

pub(crate) fn select_profile(ctx: &mut Context<HyprmuxApp>, index: usize) -> Update {
    let Some((mode, entry)) = ctx.state.profile_picker.as_ref().and_then(|picker| {
        picker
            .entries
            .get(index)
            .map(|entry| (picker.mode, entry.clone()))
    }) else {
        return Update::none();
    };

    match mode {
        ProfilePickerMode::SetDefault => match persist_default_profile(&entry.name) {
            Ok(path) => {
                ctx.state.config.profile.default = Some(entry.name.clone());
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
        },
        ProfilePickerMode::Load => {
            kill_all_live_ptys(&mut ctx.state);
            let theme_watcher = ctx.state.theme_watcher.take();
            let system_theme = ctx.state.system_theme.clone();
            let config = ctx.state.config.clone();
            let theme = ctx.state.theme.clone();

            match load_profile(&entry.path) {
                Ok(profile) => {
                    let mut new_state = State::from_profile(config, theme, profile);
                    new_state.theme_watcher = theme_watcher;
                    new_state.system_theme = system_theme;
                    ctx.state = new_state;
                    theme_ops::apply_terminal_palette_to_state(&mut ctx.state);
                    ctx.toast()
                        .push(info_toast(format!("Loaded profile `{}`", entry.name)));
                    ctx.state.show_profile_picker = false;
                    ctx.state.profile_picker = None;
                    // The theme-tick and bar-tick loops started at app launch are
                    // self-sustaining and survive the state swap, so don't restart them
                    // here — doing so would spawn duplicate loops on every load.
                    return Update::with_command(pane_lifecycle::initial_command(
                        startup_spawns(&ctx.state),
                        false,
                        false,
                    ));
                }
                Err(message) => {
                    ctx.toast().push(error_toast("Load Profile", message));
                }
            }
        }
    }

    ctx.state.show_profile_picker = false;
    ctx.state.profile_picker = None;
    Update::full()
}

fn kill_all_live_ptys(state: &mut State) {
    for workspace in &mut state.workspaces {
        for pane in &mut workspace.panes {
            if !pane.closing {
                pane.terminal.kill();
            }
        }
    }
    if let Some(scratch) = state.scratch.as_mut() {
        scratch.terminal.kill();
    }
}
