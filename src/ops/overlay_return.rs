//! Return-to-parent for nested dialogs.
//!
//! A dialog raised *from* another dialog (the theme picker from Settings, a naming prompt from a
//! picker) has to lead back where it came from: cancelling means "never mind, back to the list",
//! not "close everything". Openers of such a child record their parent in
//! [`crate::state::State::overlay_return`] — always assigning, so a standalone opening clears any
//! earlier value instead of inheriting it — and the child's close/submit paths call [`finish`] or,
//! when they complete a workflow that invalidates the parent (attaching, detaching), [`leave`].

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::ops::focus::request_current_pane_focus;
use crate::state::{OverlayOrigin, State};

/// The picker a prompt is being raised from, or `None` when it was raised standalone. Only one
/// picker can be open at a time, so the first match is the parent.
pub(crate) fn picker_origin(state: &State) -> Option<OverlayOrigin> {
    if let Some(picker) = state
        .profile_picker
        .as_ref()
        .filter(|_| state.show_profile_picker)
    {
        return Some(OverlayOrigin::ProfilePicker {
            query: picker.input.text().to_string(),
            selected: picker.selected,
            apply_mode: picker.apply_mode,
        });
    }
    if let Some(picker) = state.remote_picker.as_ref() {
        return Some(OverlayOrigin::RemoteHosts {
            query: picker.host_input.text().to_string(),
            selected_target: picker.selected_host.clone(),
        });
    }
    state
        .session_picker
        .as_ref()
        .filter(|_| state.show_session_picker)
        .map(|picker| OverlayOrigin::SessionPicker {
            query: picker.input.text().to_string(),
            selected_session: picker
                .entries
                .get(picker.selected)
                .filter(|entry| picker.in_tab(entry))
                .map(|entry| (entry.name.clone(), entry.remote_target.clone())),
            tab: picker.tab.clone(),
            browse_selected: picker.browse_selected.clone(),
        })
}

/// Reopen the dialog the current child was raised from and focus it, consuming the origin. `None`
/// when there was no parent, leaving focus for the caller to place.
///
/// A picker is rebuilt (its rows are cheap to rediscover) and then handed back the query and
/// highlighted row it had, so returning lands on the same view rather than the top of an
/// unfiltered list.
pub(crate) fn restore(ctx: &mut Context<AppRoot>) -> Option<Update> {
    match ctx.state.overlay_return.take()? {
        OverlayOrigin::Settings => {
            ctx.state.show_settings = true;
            if ctx.state.settings_selected.is_none() {
                crate::state::assign_settings_selection(
                    &mut ctx.state,
                    Some(crate::state::SettingsAction::Theme),
                );
            }
            ctx.state.commands_dirty = true;
            ctx.request_focus(crate::view::settings_palette_key());
            Some(Update::full())
        }
        OverlayOrigin::ProfilePicker {
            query,
            selected,
            apply_mode,
        } => {
            let update = if apply_mode {
                crate::ops::profile::open_apply_profile_picker(ctx)
            } else {
                crate::ops::profile::open_profile_picker(ctx)
            };
            if let Some(picker) = ctx.state.profile_picker.as_mut() {
                let cursor = query.len();
                picker.input.set_text(query);
                picker.input.set_cursor(cursor);
                picker.input.set_anchor(None);
                picker.selected = selected.min(picker.entries.len().saturating_sub(1));
            }
            Some(update)
        }
        OverlayOrigin::SessionPicker {
            query,
            selected_session,
            tab,
            browse_selected,
        } => {
            let update = crate::ops::session::open_session_picker(ctx);
            if ctx.state.is_launcher()
                && let crate::state::SessionPickerTab::Host(target) = &tab
            {
                ctx.state.launcher_scope = target.clone();
            }
            if let Some(picker) = ctx.state.session_picker.as_mut() {
                let cursor = query.len();
                picker.input.set_text(query);
                picker.input.set_cursor(cursor);
                picker.input.set_anchor(None);
                picker.tab = tab;
                picker.browse_selected = browse_selected;
                picker.selected = selected_session
                    .and_then(|(name, target)| {
                        picker
                            .entries
                            .iter()
                            .position(|entry| entry.name == name && entry.remote_target == target)
                    })
                    .unwrap_or_else(|| picker.first_in_tab().unwrap_or(0));
                picker.keep_selection_in_tab();
            }
            Some(update)
        }
        OverlayOrigin::RemoteHosts {
            query,
            selected_target,
        } => Some(crate::ops::session::remotes::restore_remote_hosts(
            ctx,
            query,
            selected_target,
        )),
    }
}

/// Close a child dialog: back to its parent when it had one, otherwise to the focused pane.
pub(crate) fn finish(ctx: &mut Context<AppRoot>) -> Update {
    match restore(ctx) {
        Some(update) => update,
        None => {
            request_current_pane_focus(ctx);
            Update::full()
        }
    }
}

/// Drop the recorded parent without reopening it, for the paths that finish a workflow the parent
/// cannot survive (attaching to a session, detaching, replacing the layout). Focus is the caller's.
pub(crate) fn leave(ctx: &mut Context<AppRoot>) {
    ctx.state.overlay_return = None;
}

#[cfg(test)]
mod tests {
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    use crate::state::{ProfilePickerState, SessionPickerState, SettingsAction};
    use crate::{AppRoot, Msg};

    /// Every case drives a real `AppRoot` through the message router, so the deep component
    /// tree needs the same roomy stack the other backend tests use.
    fn with_backend(body: impl FnOnce(&mut TestBackend<AppRoot>) + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(move || {
                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 96,
                    h: 40,
                });
                body(&mut backend);
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn theme_picker_opened_from_settings_returns_to_it() {
        with_backend(|backend| {
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsActivate(SettingsAction::Theme))
                .expect("open theme picker");
            assert!(backend.state().show_theme_picker);
            assert!(!backend.state().show_settings);

            backend
                .dispatch(Msg::CloseThemePicker)
                .expect("close theme picker");
            assert!(!backend.state().show_theme_picker);
            assert!(backend.state().show_settings);
            assert_eq!(
                backend.focused_key().map(|key| key.as_ref()),
                Some(crate::view::settings_palette_key())
            );
        });
    }

    /// Cancelling steps back into Settings, but picking a theme is the errand finished: it leaves
    /// the whole stack rather than dropping the user back into a list they are done with.
    #[test]
    fn selecting_a_theme_from_settings_closes_the_dialogs() {
        with_backend(|backend| {
            // Selecting persists the pick; `test_support` has already pointed the writer at this
            // process's scratch root rather than the developer's own config.
            backend.state_mut().show_settings = true;
            backend
                .dispatch(Msg::SettingsActivate(SettingsAction::Theme))
                .expect("open theme picker");
            backend
                .dispatch(Msg::SelectTheme(0))
                .expect("select first theme");
            let settings_closed = !backend.state().show_settings;
            let picker_closed = !backend.state().show_theme_picker;
            let origin_cleared = backend.state().overlay_return.is_none();

            assert!(picker_closed);
            assert!(settings_closed);
            assert!(origin_cleared);
        });
    }

    #[test]
    fn theme_picker_opened_standalone_closes_to_the_pane() {
        with_backend(|backend| {
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenThemePicker))
                .expect("open theme picker");
            assert!(backend.state().show_theme_picker);
            assert!(backend.state().overlay_return.is_none());

            backend
                .dispatch(Msg::CloseThemePicker)
                .expect("close theme picker");
            assert!(!backend.state().show_theme_picker);
            assert!(!backend.state().show_settings);
        });
    }

    #[test]
    fn save_profile_prompt_returns_to_the_picker_with_its_query() {
        with_backend(|backend| {
            let mut picker = ProfilePickerState::new(Vec::new());
            picker.input.set_text("web");
            backend.state_mut().profile_picker = Some(picker);
            backend.state_mut().show_profile_picker = true;

            backend
                .dispatch(Msg::ProfilePickerNew)
                .expect("open save prompt");
            assert!(backend.state().save_profile_prompt.is_some());
            assert!(!backend.state().show_profile_picker);

            backend
                .dispatch(Msg::CloseSaveProfile)
                .expect("close save prompt");
            assert!(backend.state().save_profile_prompt.is_none());
            assert!(backend.state().show_profile_picker);
            assert_eq!(
                backend
                    .state()
                    .profile_picker
                    .as_ref()
                    .map(|picker| picker.input.text().to_string()),
                Some("web".to_string())
            );
        });
    }

    #[test]
    fn create_session_prompt_returns_to_the_session_picker_with_its_query_and_tab() {
        with_backend(|backend| {
            let target = crate::session::remote::RemoteTarget::Alias("workbox".into());
            // The attached local session would reopen on Local if the origin forgot the tab.
            backend.state_mut().current_mut().session_name = Some("local-dev".into());
            backend.state_mut().current_mut().session_attached = true;
            let mut picker = SessionPickerState::new(Vec::new()).on_tab(Some(target.clone()));
            picker.input.set_text("dev");
            backend.state_mut().session_picker = Some(picker);
            backend.state_mut().show_session_picker = true;

            backend
                .dispatch(Msg::SessionPickerCreateFromQuery)
                .expect("open create prompt");
            assert!(!backend.state().show_session_picker);
            // Ctrl+N from a query means "then make that one": the name comes along rather than
            // being typed a second time.
            assert_eq!(
                backend
                    .state()
                    .rename_session
                    .as_ref()
                    .map(|rename| rename.input.text().to_string()),
                Some("dev".to_string())
            );

            backend
                .dispatch(Msg::CloseRenameSession)
                .expect("close create prompt");
            assert!(backend.state().rename_session.is_none());
            assert!(backend.state().show_session_picker);
            assert_eq!(
                backend
                    .state()
                    .session_picker
                    .as_ref()
                    .map(|picker| picker.input.text().to_string()),
                Some("dev".to_string())
            );
            assert_eq!(
                backend.state().session_picker.as_ref().unwrap().tab,
                crate::state::SessionPickerTab::Host(Some(target))
            );
        });
    }

    #[test]
    fn remote_hosts_restores_the_browsed_session_identity_after_rows_change() {
        with_backend(|backend| {
            let attached = crate::session::remote::RemoteTarget::Alias("host-a".into());
            let browsed = crate::session::remote::RemoteTarget::Alias("host-b".into());
            let current = backend.state_mut().current_mut();
            current.session_name = Some("dev".into());
            current.session_attached = true;
            current.remote_target = Some(attached.clone());
            current.remote_host = Some("host-a".into());
            let row = crate::session::discovery::DiscoveredSession {
                name: "backend".into(),
                origin: Default::default(),
                ephemeral: false,
                host: Some("host-b".into()),
                remote_target: Some(browsed.clone()),
                status: crate::session::discovery::DiscoveredSessionStatus::Running {
                    panes: 1,
                    has_layout: true,
                    clients: 1,
                },
            };
            backend.state_mut().remote.live_sessions = vec![row.clone()];
            let mut picker =
                SessionPickerState::new(vec![row.clone()]).on_tab(Some(browsed.clone()));
            picker.input.set_text("backend");
            backend.state_mut().session_picker = Some(picker);
            backend.state_mut().show_session_picker = true;

            backend
                .dispatch(Msg::SessionPickerRemoteHosts)
                .expect("open remote hosts");
            // Discovery adds an earlier row on the browsed host and a same-name session on the
            // attached host. Neither the old index nor the name alone identifies the selection.
            let mut earlier = row.clone();
            earlier.name = "alpha".into();
            let mut other_host = row.clone();
            other_host.host = Some("host-a".into());
            other_host.remote_target = Some(attached.clone());
            backend.state_mut().remote.live_sessions =
                vec![earlier.clone(), other_host, row.clone()];
            backend
                .dispatch(Msg::CloseRemotePicker)
                .expect("return to sessions");

            assert!(backend.state().show_session_picker);
            let picker = backend.state().session_picker.as_ref().unwrap();
            assert_eq!(
                picker.tab,
                crate::state::SessionPickerTab::Host(Some(browsed))
            );
            assert_eq!(picker.input.text(), "backend");
            assert_eq!(backend.state().current().remote_target, Some(attached));
            let selected = &picker.entries[picker.selected];
            assert_eq!(selected.name, "backend");
            assert!(picker.in_tab(selected));

            backend
                .dispatch(Msg::SessionPickerRemoteHosts)
                .expect("open remote hosts again");
            // Search stays on the restored host. Clearing it falls back to that host's
            // first row if the old selection disappeared.
            backend.state_mut().remote.live_sessions = vec![earlier];
            backend
                .dispatch(Msg::CloseRemotePicker)
                .expect("return after the selected session disappears");
            let picker = backend.state().session_picker.as_ref().unwrap();
            assert_eq!(
                picker.tab,
                crate::state::SessionPickerTab::Host(row.remote_target)
            );
            assert_eq!(picker.input.text(), "backend");
            backend
                .dispatch(Msg::SessionPickerQueryChanged(String::new()))
                .unwrap();
            let picker = backend.state().session_picker.as_ref().unwrap();
            assert_eq!(picker.entries[picker.selected].name, "alpha");
            assert!(picker.in_tab(&picker.entries[picker.selected]));
        });
    }

    #[test]
    fn remote_create_prompt_returns_to_the_shared_host_tab() {
        with_backend(|backend| {
            let target = crate::session::remote::RemoteTarget::Alias("workbox".into());
            let mut sessions = SessionPickerState::new(Vec::new()).on_tab(Some(target.clone()));
            sessions.input.set_text("new-name");
            backend.state_mut().session_picker = Some(sessions);
            backend.state_mut().show_session_picker = true;

            backend
                .dispatch(Msg::SessionPickerCreateFromQuery)
                .expect("open remote create prompt");
            assert_eq!(
                backend
                    .state()
                    .rename_session
                    .as_ref()
                    .unwrap()
                    .host_target
                    .as_ref(),
                Some(&target)
            );
            backend
                .dispatch(Msg::CloseRenameSession)
                .expect("return to the shared host tab");
            let picker = backend.state().session_picker.as_ref().unwrap();
            assert!(backend.state().show_session_picker);
            assert!(backend.state().remote_picker.is_none());
            assert_eq!(picker.tab.remote_target(), Some(&target));
            assert_eq!(picker.input.text(), "new-name");
            backend.dispatch(Msg::CloseSessionPicker).unwrap();
            assert!(!backend.state().show_session_picker);
            assert!(backend.state().remote_picker.is_none());
        });
    }

    /// A prompt that is never nested must clear whatever origin an earlier child left behind,
    /// rather than inheriting it and reopening an unrelated dialog on cancel.
    #[test]
    fn standalone_prompt_does_not_inherit_a_stale_origin() {
        with_backend(|backend| {
            backend.state_mut().overlay_return = Some(super::OverlayOrigin::Settings);
            backend
                .dispatch(Msg::RunAction(crate::input::Action::RenameWorkspace))
                .expect("open workspace rename");
            assert!(backend.state().overlay_return.is_none());

            backend
                .dispatch(Msg::CloseRenameSession)
                .expect("close workspace rename");
            assert!(!backend.state().show_settings);
        });
    }
}
