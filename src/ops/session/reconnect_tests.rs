use crate::session::remote::RemoteTarget;
use crate::state::{HostProbe, RemotePickerState};
use crate::{AppRoot, Msg};
use tui_lipan::TestBackend;

fn with_backend(body: impl FnOnce(&mut TestBackend<AppRoot>) + Send + 'static) {
    crate::test_support::on_large_stack(move || {
        crate::test_support::isolate_user_dirs();
        let _persist = crate::test_support::lock_persisted_state();
        let _restore = crate::test_support::preserve_persisted_state();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().command_link = None;
        body(&mut backend);
    });
}

fn picker(backend: &mut TestBackend<AppRoot>, target: &RemoteTarget, probe: HostProbe) {
    let state = backend.state_mut();
    state
        .remote
        .hosts
        .seed(&state.config.remote, std::slice::from_ref(target), &[], &[]);
    state.remote.hosts.get_mut(target).unwrap().probe = probe;
    state.remote_picker = Some(RemotePickerState::new(Some(target.clone())));
}

fn stored(target: &RemoteTarget) -> bool {
    crate::session::read_reconnect_hosts().contains(target)
}

fn probe_result(backend: &mut TestBackend<AppRoot>, target: &RemoteTarget, success: bool) {
    let epoch = backend.state().remote_picker.as_ref().unwrap().probe_epoch;
    backend
        .update_level(Msg::RemoteHostSessionsDiscovered {
            epoch,
            target: target.clone(),
            rows: if success {
                Ok(Vec::new())
            } else {
                Err("Connection timed out".into())
            },
        })
        .unwrap();
}

fn monitor_result(backend: &mut TestBackend<AppRoot>, target: &RemoteTarget) {
    backend
        .state_mut()
        .remote
        .monitors
        .push(crate::session::remote::monitor::Monitor::dormant(
            target.clone(),
            77,
        ));
    backend
        .update_level(Msg::HostMetadata {
            target: target.clone(),
            generation: 77,
            rows: Ok(Vec::new()),
            agents: Vec::new(),
            projects: Vec::new(),
        })
        .unwrap();
}

#[test]
fn explicit_connection_persists_after_probe_or_after_a_failed_probe_then_monitor_success() {
    with_backend(|backend| {
        for through_monitor in [false, true] {
            let target = RemoteTarget::Alias(format!("intent-success-{through_monitor}.invalid"));
            picker(backend, &target, HostProbe::Idle);
            backend
                .update_level(Msg::RemotePickerHostActivate(target.clone()))
                .unwrap();
            assert!(backend.state().remote.remember_on_reach.contains(&target));
            assert!(!stored(&target));
            if through_monitor {
                probe_result(backend, &target, false);
                assert!(backend.state().remote.remember_on_reach.contains(&target));
                assert!(!stored(&target));
                monitor_result(backend, &target);
            } else {
                probe_result(backend, &target, true);
            }
            assert!(stored(&target));
            assert!(!backend.state().remote.remember_on_reach.contains(&target));
        }
    });
}

#[test]
fn connected_enter_and_refresh_store_immediately() {
    with_backend(|backend| {
        for refresh in [false, true] {
            let target = RemoteTarget::Alias(format!("intent-connected-{refresh}.invalid"));
            picker(backend, &target, HostProbe::Reached);
            backend
                .update_level(if refresh {
                    Msg::RemotePickerReconnectHost
                } else {
                    Msg::RemotePickerHostActivate(target.clone())
                })
                .unwrap();
            assert!(stored(&target));
            assert!(!backend.state().remote.remember_on_reach.contains(&target));
        }
    });
}

#[test]
fn startup_activation_and_background_success_never_create_intent() {
    with_backend(|backend| {
        let target = RemoteTarget::Alias("intent-command-line.invalid".into());
        picker(backend, &target, HostProbe::Idle);
        backend
            .update_level(Msg::RemoteStartupActivate(target.clone()))
            .unwrap();
        assert!(!backend.state().remote.remember_on_reach.contains(&target));
        probe_result(backend, &target, true);
        assert!(!stored(&target));
        monitor_result(backend, &target);
        assert!(!stored(&target));
    });
}

#[test]
fn cancellation_disarms_but_keeps_a_stored_entry() {
    with_backend(|backend| {
        for remembered in [false, true] {
            let target = RemoteTarget::Alias(format!("intent-cancel-{remembered}.invalid"));
            if remembered {
                crate::session::add_reconnect_host(&target).unwrap();
            }
            picker(backend, &target, HostProbe::Idle);
            backend
                .update_level(Msg::RemotePickerHostActivate(target.clone()))
                .unwrap();
            let epoch = backend.state().remote_picker.as_ref().unwrap().probe_epoch;
            backend.update_level(Msg::CloseRemotePicker).unwrap();
            assert!(!backend.state().remote.remember_on_reach.contains(&target));
            assert_eq!(stored(&target), remembered);
            backend
                .update_level(Msg::RemoteHostSessionsDiscovered {
                    epoch,
                    target: target.clone(),
                    rows: Ok(Vec::new()),
                })
                .unwrap();
            assert_eq!(
                stored(&target),
                remembered,
                "abandoned probe cannot record intent"
            );
        }
    });
}

#[test]
fn adding_and_editing_move_intent_without_remembering_an_unreached_host() {
    with_backend(|backend| {
        let old = RemoteTarget::Alias("intent-added.invalid".into());
        let new = RemoteTarget::Alias("intent-edited.invalid".into());
        picker(backend, &old, HostProbe::Idle);
        backend
            .state_mut()
            .remote_picker
            .as_mut()
            .unwrap()
            .host_form = Some(crate::state::HostFormState::add(old.to_spec()));
        backend.update_level(Msg::SubmitHostForm).unwrap();
        assert!(backend.state().remote.remember_on_reach.contains(&old));
        assert!(!stored(&old));
        probe_result(backend, &old, false);
        crate::session::add_reconnect_host(&old).unwrap();
        backend.state_mut().remote.reconnect_hosts.push(old.clone());
        let mut form = crate::state::HostFormState::edit(&old);
        form.host.set_text(new.to_spec());
        backend
            .state_mut()
            .remote_picker
            .as_mut()
            .unwrap()
            .host_form = Some(form);
        backend.update_level(Msg::SubmitHostForm).unwrap();
        assert!(!stored(&old));
        assert!(stored(&new));
        assert!(!backend.state().remote.remember_on_reach.contains(&old));
        assert!(backend.state().remote.remember_on_reach.contains(&new));
        assert!(backend.state().remote.reconnect_hosts.contains(&new));
    });
}

#[test]
fn forgetting_removes_and_disarms_intent() {
    with_backend(|backend| {
        let target = RemoteTarget::Alias("intent-forgotten.invalid".into());
        crate::session::save_host(&target).unwrap();
        crate::session::add_reconnect_host(&target).unwrap();
        picker(backend, &target, HostProbe::Failed("timeout".into()));
        backend
            .state_mut()
            .remote
            .reconnect_hosts
            .push(target.clone());
        backend.state_mut().remote.arm_reconnect(&target);
        backend.update_level(Msg::RemotePickerForgetHost).unwrap();
        backend.update_level(Msg::RemotePickerForgetHost).unwrap();
        assert!(!stored(&target));
        assert!(!backend.state().remote.remember_on_reach.contains(&target));
        assert!(!backend.state().remote.reconnect_hosts.contains(&target));
    });
}

/// The file is what causes a connection at the next launch, so a removal that did not reach it has
/// to say so, while this client still lets go of the host.
#[cfg(unix)]
#[test]
fn a_disconnect_whose_removal_cannot_be_saved_warns_and_still_disconnects() {
    use std::os::unix::fs::PermissionsExt;
    with_backend(|backend| {
        use crate::state::{SessionPickerState, SessionPickerTab};
        let target = RemoteTarget::Alias("intent-unsaved.invalid".into());
        // A link into a directory that refuses new files: the entry stays readable, and the
        // atomic replace, which writes a sibling of the link's target, fails.
        let locked = tempfile::tempdir().unwrap();
        let held = locked.path().join("reconnect-hosts");
        std::fs::write(&held, target.to_spec()).unwrap();
        let link = crate::platform::paths::private_state_dir(
            &crate::platform::paths::PlatformEnv::from_process(),
        )
        .unwrap()
        .join("reconnect-hosts");
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&held, &link).unwrap();
        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o500)).unwrap();
        // Permissions do not bind root, which leaves nothing here to observe.
        let refused = std::fs::File::create(locked.path().join("probe")).is_err();

        picker(backend, &target, HostProbe::Reached);
        backend.state_mut().remote_picker = None;
        backend
            .state_mut()
            .remote
            .reconnect_hosts
            .push(target.clone());
        let mut sessions = SessionPickerState::new(Vec::new());
        sessions.tab = SessionPickerTab::Host(Some(target.clone()));
        backend.state_mut().session_picker = Some(sessions);
        backend.state_mut().show_session_picker = true;
        backend.state_mut().remote.monitors.push(
            crate::session::remote::monitor::Monitor::dormant(target.clone(), 91),
        );
        backend
            .update_level(Msg::SessionPickerDisconnectHost)
            .unwrap();

        if refused {
            assert!(stored(&target), "the entry could not be removed");
            assert!(
                backend
                    .state()
                    .replaceable_toasts
                    .values()
                    .any(|toast| toast.content().contains("reconnect preference not saved")),
                "the failure is reported"
            );
        }
        let state = backend.state();
        assert!(!state.remote.reconnect_hosts.contains(&target));
        assert!(
            state
                .remote
                .monitors
                .iter()
                .all(|monitor| monitor.target != target)
        );
        assert_eq!(
            state.remote.hosts.get(&target).unwrap().probe,
            HostProbe::Idle
        );

        std::fs::set_permissions(locked.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::remove_file(&link).unwrap();
    });
}

#[test]
fn sidebar_connect_arms_and_monitor_only_disconnect_removes_intent() {
    with_backend(|backend| {
        use crate::config::SidebarTab;
        use crate::state::{RowTarget, SessionPickerState, SessionPickerTab};
        for use_sidebar in [false, true] {
            let target = RemoteTarget::Alias(format!("intent-sidebar-{use_sidebar}.invalid"));
            crate::session::save_host(&target).unwrap();
            picker(backend, &target, HostProbe::Idle);
            backend.state_mut().remote_picker = None;
            backend.state_mut().sidebar.panels[0].active_tab =
                Some(crate::config::SidebarTabId::new("sessions"));
            let rows = backend.state().sidebar_rows(&SidebarTab::Sessions);
            let index = rows
                .iter()
                .position(|row| row.target == RowTarget::HostConnect(target.clone()))
                .unwrap();
            backend
                .update_level(Msg::SidebarRowActivate { panel: 0, index })
                .unwrap();
            assert!(backend.state().remote.remember_on_reach.contains(&target));
            monitor_result(backend, &target);
            assert!(stored(&target));
            backend
                .state_mut()
                .remote
                .hosts
                .get_mut(&target)
                .unwrap()
                .probe = HostProbe::Failed("timeout".into());
            let rows = backend.state().sidebar_rows(&SidebarTab::Sessions);
            assert!(rows.iter().any(|row| row.close
                == Some(crate::state::SidebarClose::Host {
                    target: target.clone()
                })));
            if use_sidebar {
                let index = rows
                    .iter()
                    .position(|row| {
                        row.close
                            == Some(crate::state::SidebarClose::Host {
                                target: target.clone(),
                            })
                    })
                    .unwrap();
                backend
                    .update_level(Msg::SidebarRowClose { panel: 0, index })
                    .unwrap();
                backend
                    .update_level(Msg::SidebarRowClose { panel: 0, index })
                    .unwrap();
            } else {
                let mut sessions = SessionPickerState::new(Vec::new());
                sessions.tab = SessionPickerTab::Host(Some(target.clone()));
                backend.state_mut().session_picker = Some(sessions);
                backend.state_mut().show_session_picker = true;
                backend
                    .update_level(Msg::SessionPickerDisconnectHost)
                    .unwrap();
            }
            assert!(!stored(&target));
            assert!(!backend.state().remote.remember_on_reach.contains(&target));
            assert!(
                backend
                    .state()
                    .remote
                    .monitors
                    .iter()
                    .all(|monitor| monitor.target != target)
            );
            assert_eq!(
                backend.state().remote.hosts.get(&target).unwrap().probe,
                HostProbe::Idle
            );
        }
    });
}
