//! The session picker's route to this client's scratch (ephemeral) session. The key always works;
//! the footer only spends a pill on it when the list cannot point the way itself.

use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::state::SessionPickerState;
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::*;

const VIEWPORT: Rect = Rect {
    x: 0,
    y: 0,
    w: 100,
    h: 30,
};

fn session_row(name: &str) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        origin: Default::default(),
        ephemeral: false,
        host: None,
        remote_target: None,
        status: DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
        },
    }
}

/// Rendering the app recurses deeply enough to overflow a default test stack.
fn on_a_big_stack(body: impl FnOnce() + Send + 'static) {
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(move || {
            rozi::test_support::isolate_user_dirs();
            body();
        })
        .expect("spawn render thread")
        .join()
        .expect("render thread completes");
}

fn screen(backend: &mut TestBackend<AppRoot>) -> String {
    backend.render();
    backend.advance(std::time::Duration::from_millis(200));
    backend.capture_frame().plain_text()
}

fn disable_background_commands(backend: &mut TestBackend<AppRoot>) {
    // Consume CommandLinkReady before clearing the link, so late startup cannot launch host
    // monitors against the fake hosts these picker tests install.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while backend.state().command_link.is_none() {
        assert!(
            std::time::Instant::now() < deadline,
            "startup did not finish"
        );
        backend.pump().unwrap();
        std::thread::yield_now();
    }
    backend.state_mut().command_link = None;
}

#[test]
fn nothing_to_pick_puts_the_scratch_session_on_enter() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(Vec::new()));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("│ No sessions"),
            "empty-state copy keeps the same 1-cell left inset as the search field:\n{rendered}"
        );
        assert!(
            !rendered.contains("│No sessions"),
            "empty-state copy must not sit flush against the frame:\n{rendered}"
        );
        assert!(
            rendered.contains("ephemeral shell Enter"),
            "with no row to activate, Enter carries the scratch session:\n{rendered}"
        );
        assert!(
            !rendered.contains("ephemeral shell Ctrl+T"),
            "the chord goes unsaid while Enter already offers it:\n{rendered}"
        );

        // The key itself, not just the message it sends: the palette must let a bare Enter
        // through once it has no row of its own to activate.
        backend
            .send_key(KeyEvent {
                code: KeyCode::Enter,
                mods: KeyMods::NONE,
            })
            .expect("press enter on the empty picker");
        let state = backend.state();
        assert!(!state.show_session_picker);
        assert_eq!(
            state
                .current()
                .pending_session_attach
                .as_ref()
                .map(|pending| pending.name.as_str()),
            Some(rozi::state::ephemeral_session_name().as_str())
        );
    });
}

#[test]
fn unmatched_queries_offer_recovery_actions_and_leave_enter_inactive() {
    on_a_big_stack(|| {
        for target in [
            None,
            Some(rozi::session::remote::RemoteTarget::Alias("workbox".into())),
        ] {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(VIEWPORT);
            {
                let state = backend.state_mut();
                *state.current_mut() = rozi::state::Attachment::new();
                state.current_mut().session_name = Some(rozi::state::ephemeral_session_name());
                state.current_mut().session_attached = true;
                state.show_session_picker = true;
                let mut picker =
                    SessionPickerState::new(vec![session_row("dev")]).on_tab(target.clone());
                picker.input.set_text("Efefef");
                state.session_picker = Some(picker);
            }
            let rendered = screen(&mut backend);
            assert!(
                rendered.contains("No sessions match `Efefef`"),
                "{rendered}"
            );
            let host = target.as_ref().map_or("Local", |_| "workbox");
            for hint in [
                format!("new on {host} Ctrl+N"),
                format!("ephemeral shell on {host} Ctrl+T"),
                "name current Ctrl+S".into(),
                "remote hosts Ctrl+R".into(),
            ] {
                assert!(rendered.contains(&hint), "missing {hint}: {rendered}");
            }
            assert!(!rendered.contains("shell Enter"), "{rendered}");
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Enter,
                    mods: KeyMods::NONE,
                })
                .unwrap();
            assert!(backend.state().show_session_picker);
            assert!(backend.state().current().pending_session_attach.is_none());
            assert!(backend.state().rename_session.is_none());
            assert_eq!(
                backend
                    .state()
                    .session_picker
                    .as_ref()
                    .unwrap()
                    .input
                    .text(),
                "Efefef"
            );

            for (key, mode) in [
                ('n', rozi::state::NamingMode::CreateSession),
                ('s', rozi::state::NamingMode::NameEphemeralSession),
            ] {
                screen(&mut backend);
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char(key),
                        mods: KeyMods::CTRL,
                    })
                    .unwrap();
                let prompt = backend.state().rename_session.as_ref().unwrap();
                assert_eq!(prompt.input.text(), "Efefef");
                assert_eq!(prompt.mode, mode);
                if key == 'n' {
                    assert_eq!(prompt.host_target, target);
                }
                backend.dispatch(Msg::CloseRenameSession).unwrap();
                let picker = backend.state().session_picker.as_ref().unwrap();
                assert_eq!(picker.input.text(), "Efefef");
                assert_eq!(picker.tab.remote_target(), target.as_ref());
            }
            screen(&mut backend);
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char('r'),
                    mods: KeyMods::CTRL,
                })
                .unwrap();
            assert!(backend.state().remote_picker.is_some());
            backend.dispatch(Msg::CloseRemotePicker).unwrap();
            let picker = backend.state().session_picker.as_ref().unwrap();
            assert_eq!(picker.input.text(), "Efefef");
            assert_eq!(picker.tab.remote_target(), target.as_ref());
            if target.is_none() {
                screen(&mut backend);
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('t'),
                        mods: KeyMods::CTRL,
                    })
                    .unwrap();
                assert!(!backend.state().show_session_picker);
            }
        }
    });
}

#[test]
fn a_populated_list_advertises_the_chord_until_the_scratch_session_exists() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("ephemeral shell Ctrl+T"),
            "with rows on the list, Enter belongs to them and the chord is spelled out:\n{rendered}"
        );
        assert!(
            !rendered.contains("ephemeral shell Enter"),
            "Enter stays the list's own key:\n{rendered}"
        );
    });
}

#[test]
fn padded_selection_insets_the_current_marker() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            let session_name = rozi::state::ephemeral_session_name();
            state.current_mut().session_name = Some(session_name.clone());
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![DiscoveredSession {
                name: session_name,
                origin: Default::default(),
                ephemeral: true,
                host: None,
                remote_target: None,
                status: DiscoveredSessionStatus::Running {
                    panes: 1,
                    has_layout: true,
                    clients: 1,
                },
            }]));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("│ ● ephemeral"),
            "padded selection keeps a 1-cell inset before the status marker:\n{rendered}"
        );
        assert!(
            !rendered.contains("│● ephemeral"),
            "padded selection must not sit flush against the frame:\n{rendered}"
        );
    });
}

#[test]
fn holding_the_scratch_session_drops_the_hint_but_not_the_key() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some(rozi::state::ephemeral_session_name());
            state.current_mut().session_attached = true;
            // Startup queued its own attach; this client is meant to be settled on the session.
            state.current_mut().pending_session_attach = None;
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("ephemeral shell Ctrl+T")
                && !rendered.contains("ephemeral shell Enter"),
            "the scratch session is on the list itself, so the pill would be noise:\n{rendered}"
        );

        backend
            .dispatch(Msg::SessionPickerEphemeral)
            .expect("the chord still answers");
        let state = backend.state();
        assert!(
            !state.show_session_picker,
            "asking for the session you are already on closes the picker"
        );
        assert!(
            state.current().pending_session_attach.is_none(),
            "and does not re-attach what is already attached"
        );
    });
}

#[test]
fn a_parked_scratch_session_also_drops_the_hint() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            let mut parked = rozi::state::Attachment::new();
            parked.session_name = Some(rozi::state::ephemeral_session_name());
            parked.session_attached = true;
            state.background.insert(7, parked);
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev")]));
        }

        let rendered = screen(&mut backend);
        assert!(
            !rendered.contains("ephemeral shell Ctrl+T"),
            "a scratch session parked in the background is one the client already has:\n{rendered}"
        );
    });
}

fn remote_row(name: &str, host: &str) -> DiscoveredSession {
    DiscoveredSession {
        host: Some(host.to_string()),
        remote_target: Some(rozi::session::remote::RemoteTarget::Alias(host.to_string())),
        ..session_row(name)
    }
}

/// With a host in play the picker splits into tabs, one per machine. Each tab lists only its own
/// sessions, and a sessionless client opens on the tab of the host its launcher names.
#[test]
fn a_host_in_play_gets_its_own_tab_and_the_launcher_opens_on_it() {
    use rozi::input::Action;
    on_a_big_stack(|| {
        let workbox = rozi::session::remote::RemoteTarget::Alias("workbox".to_string());
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.launcher_scope = Some(workbox.clone());
        }
        backend
            .dispatch(Msg::RunAction(Action::OpenSessionPicker))
            .expect("open Sessions");
        backend
            .state_mut()
            .session_picker
            .as_mut()
            .expect("picker")
            .entries = vec![session_row("dev"), remote_row("api", "workbox")];

        let on_host = screen(&mut backend);
        let search_row = on_host
            .lines()
            .position(|line| line.contains("Search sessions"))
            .expect("search field");
        let tabs_row = on_host
            .lines()
            .position(|line| line.contains("Local") && line.contains("workbox"))
            .expect("host tabs");
        let result_row = on_host
            .lines()
            .position(|line| line.contains("api"))
            .expect("session row");
        assert!(
            search_row < tabs_row && tabs_row < result_row,
            "search, tabs, then results:\n{on_host}"
        );
        assert!(
            on_host.contains("Local") && on_host.contains("workbox"),
            "one tab per machine:\n{on_host}"
        );
        assert!(
            on_host.contains("api") && !on_host.contains("dev"),
            "the launcher's host is the tab it opens on, and that tab lists only its rows:\n{on_host}"
        );

        backend
            .dispatch(Msg::SessionPickerTab(0))
            .expect("switch to Local");
        let local = screen(&mut backend);
        assert!(
            local.contains("dev") && !local.contains("api"),
            "Local lists this machine's sessions:\n{local}"
        );
        assert_eq!(
            backend.state().launcher_scope,
            None,
            "in the launcher, the tab is the scope, so the card behind it follows"
        );
    });
}

/// The creating keys act on the tab on screen. In a launcher whose picker is on a host's tab, the
/// shell starts on that host, which the tab and the card behind it both name.
#[test]
fn the_scratch_key_starts_its_shell_on_the_active_tabs_host() {
    use rozi::input::Action;
    on_a_big_stack(|| {
        let workbox = rozi::session::remote::RemoteTarget::Alias("workbox".to_string());
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.launcher_scope = Some(workbox.clone());
        }
        backend
            .dispatch(Msg::RunAction(Action::OpenSessionPicker))
            .expect("open Sessions");
        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("ephemeral shell Enter") && !rendered.contains("local"),
            "the tab names the host, so the key needs no qualifier:\n{rendered}"
        );

        backend
            .dispatch(Msg::SessionPickerEphemeral)
            .expect("start the scratch session");
        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("an attach is in flight");
        assert_eq!(pending.remote_host.as_deref(), Some("workbox"));
        assert_eq!(state.current().remote_target.as_ref(), Some(&workbox));
    });
}

/// A sessionless client scoped to a host is a real resting state: it names the machine, holds no
/// session, and starts its shell there.
#[test]
fn a_scoped_launcher_names_its_host_and_says_where_its_shell_lands() {
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.launcher_scope = Some(rozi::session::remote::RemoteTarget::Alias(
                "workbox".to_string(),
            ));
        }

        let rendered = screen(&mut backend);
        assert!(
            rendered.contains("REMOTE · workbox"),
            "the launcher wears the scope it will act in:\n{rendered}"
        );
        assert!(
            rendered.contains("shell on workbox"),
            "and its shell row says where the shell lands:\n{rendered}"
        );
    });
}

#[test]
fn search_filters_only_the_selected_host_or_all() {
    use rozi::state::SessionPickerTab;
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        disable_background_commands(&mut backend);
        {
            let state = backend.state_mut();
            state.config.animations.picker = rozi::layout::anim::PickerAnimationStyle::Off;
            state.show_session_picker = true;
            let hosts = ["workbox", "buildbox"]
                .map(|host| rozi::session::remote::RemoteTarget::Alias(host.into()));
            state.remote.added_hosts = hosts.to_vec();
            state
                .remote
                .hosts
                .seed(&state.config.remote, &hosts, &[], &[]);
            for target in &hosts {
                state.remote.hosts.get_mut(target).unwrap().probe = rozi::state::HostProbe::Reached;
            }
            state.session_picker = Some(SessionPickerState::new(vec![
                session_row("dev"),
                remote_row("dev", "workbox"),
                remote_row("api", "buildbox"),
            ]));
        }
        let local = screen(&mut backend);
        assert!(local.contains("All") && local.contains("Local"), "{local}");
        assert!(!local.contains("dev@workbox"), "{local}");
        let tabs = local
            .lines()
            .find(|line| {
                line.contains("Local") && line.contains("buildbox") && line.contains("workbox")
            })
            .unwrap();
        assert!(
            tabs.find("Local") < tabs.find("buildbox")
                && tabs.find("buildbox") < tabs.find("workbox")
                && tabs.find("workbox") < tabs.find("All"),
            "{tabs}"
        );
        backend.dispatch(Msg::SessionPickerTab(3)).unwrap();
        let all = screen(&mut backend);
        assert!(
            all.contains("dev@workbox") && all.contains("api@buildbox"),
            "{all}"
        );
        backend.dispatch(Msg::SessionPickerTab(0)).unwrap();
        screen(&mut backend);
        for ch in "dev@workbox".chars() {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char(ch),
                    mods: KeyMods::NONE,
                })
                .unwrap();
        }
        let found = screen(&mut backend);
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(picker.tab, SessionPickerTab::Host(None));
        assert!(found.contains("No sessions match"), "{found}");
        for _ in "dev@workbox".chars() {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Backspace,
                    mods: KeyMods::NONE,
                })
                .unwrap();
        }
        let cleared = screen(&mut backend);
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(picker.tab, SessionPickerTab::Host(None));
        assert_eq!(picker.selected, 0);
        assert!(!cleared.contains("dev@workbox"), "{cleared}");
        // Search on a remote tab must not reveal a matching row on another host.
        backend.dispatch(Msg::SessionPickerTab(2)).unwrap();
        screen(&mut backend);
        backend
            .dispatch(Msg::SessionPickerQueryChanged("api".into()))
            .unwrap();
        let scoped = screen(&mut backend);
        assert!(!scoped.contains("api@buildbox"), "{scoped}");
        assert!(scoped.contains("No sessions match"), "{scoped}");
        assert_eq!(
            backend
                .state()
                .session_picker
                .as_ref()
                .unwrap()
                .tab
                .remote_target(),
            Some(&rozi::session::remote::RemoteTarget::Alias(
                "workbox".into()
            ))
        );
        backend.dispatch(Msg::SessionPickerTab(3)).unwrap();
        screen(&mut backend);
        backend
            .dispatch(Msg::SessionPickerQueryChanged("api".into()))
            .unwrap();
        let global = screen(&mut backend);
        assert!(global.contains("api@buildbox"), "{global}");
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(picker.tab, SessionPickerTab::All);
        assert_eq!(picker.entries[picker.selected].name, "api");
        backend.dispatch(Msg::SessionPickerTab(0)).unwrap();
        screen(&mut backend);
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char('x'),
                mods: KeyMods::NONE,
            })
            .unwrap();
        assert_eq!(
            backend
                .state()
                .session_picker
                .as_ref()
                .unwrap()
                .input
                .text(),
            "x"
        );
    });
}

#[test]
fn choosing_a_host_tab_clears_search_and_keeps_creation_on_that_host() {
    use rozi::session::remote::RemoteTarget;
    use rozi::state::SessionPickerTab;
    on_a_big_stack(|| {
        let workbox = RemoteTarget::Alias("workbox".into());
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(VIEWPORT);
        {
            let state = backend.state_mut();
            *state.current_mut() = rozi::state::Attachment::new();
            state.launcher_scope = Some(workbox.clone());
            state.show_session_picker = true;
            state.session_picker = Some(
                SessionPickerState::new(vec![session_row("dev"), remote_row("api", "workbox")])
                    .on_tab(Some(workbox.clone())),
            );
        }
        backend
            .dispatch(Msg::SessionPickerQueryChanged("dev".into()))
            .unwrap();
        let scoped_search = screen(&mut backend);
        assert!(!scoped_search.contains("dev@"), "{scoped_search}");
        assert_eq!(backend.state().launcher_scope, Some(workbox.clone()));
        backend.dispatch(Msg::SessionPickerTab(2)).unwrap();
        assert_eq!(backend.state().launcher_scope, Some(workbox.clone()));
        backend.dispatch(Msg::SessionPickerTab(1)).unwrap();
        let scoped = screen(&mut backend);
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(picker.tab, SessionPickerTab::Host(Some(workbox.clone())));
        assert!(picker.input.text().is_empty());
        assert!(!scoped.contains("api@workbox"), "{scoped}");
        assert!(scoped.contains("api"), "{scoped}");
        backend.dispatch(Msg::SessionPickerTab(2)).unwrap();
        backend.dispatch(Msg::SessionPickerCreateFromQuery).unwrap();
        assert_eq!(
            backend.state().rename_session.as_ref().unwrap().host_target,
            Some(workbox.clone())
        );
        backend.dispatch(Msg::CloseRenameSession).unwrap();
    });
}

#[test]
fn colliding_remote_targets_have_distinct_session_tabs_rows_search_and_creation() {
    use rozi::session::remote::RemoteTarget;
    use rozi::state::SessionPickerTab;
    on_a_big_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 180,
            h: 30,
        });
        disable_background_commands(&mut backend);
        let alias = RemoteTarget::Alias("workbox".into());
        let url = RemoteTarget::Url {
            user: None,
            host: "workbox".into(),
            port: None,
        };
        let alias_row = remote_row("dev", "workbox");
        let mut url_row = alias_row.clone();
        url_row.remote_target = Some(url.clone());
        {
            let state = backend.state_mut();
            state.config.animations.picker = rozi::layout::anim::PickerAnimationStyle::Off;
            state.show_session_picker = true;
            state.remote.added_hosts = vec![alias.clone(), url.clone()];
            state.remote.hosts.seed(
                &state.config.remote,
                &[alias.clone(), url.clone()],
                &[],
                &[],
            );
            for target in [&alias, &url] {
                state.remote.hosts.get_mut(target).unwrap().probe = rozi::state::HostProbe::Reached;
            }
            let mut picker = SessionPickerState::new(vec![alias_row, url_row]);
            picker.tab = SessionPickerTab::All;
            state.session_picker = Some(picker);
        }
        let all = screen(&mut backend);
        assert!(all.contains("dev@workbox (workbox)"), "{all}");
        assert!(all.contains("dev@workbox (ssh://workbox)"), "{all}");
        let tabs = all
            .lines()
            .find(|line| line.contains("Local") && line.contains("All"))
            .unwrap();
        assert!(
            tabs.contains("workbox (workbox)") && tabs.contains("workbox (ssh://workbox)"),
            "{tabs}"
        );
        for ch in "ssh://workbox".chars() {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char(ch),
                    mods: KeyMods::NONE,
                })
                .unwrap();
        }
        let found = screen(&mut backend);
        assert!(found.contains("dev@workbox (ssh://workbox)"), "{found}");
        assert!(!found.contains("dev@workbox (workbox)"), "{found}");
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(
            picker.entries[picker.selected].remote_target,
            Some(url.clone())
        );
        // Equal display names sort by their exact specs.
        backend.dispatch(Msg::SessionPickerTab(2)).unwrap();
        screen(&mut backend);
        assert_eq!(
            backend.state().session_picker.as_ref().unwrap().tab,
            SessionPickerTab::Host(Some(alias))
        );
        backend.dispatch(Msg::SessionPickerTab(1)).unwrap();
        for ch in "dev@ssh://workbox".chars() {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char(ch),
                    mods: KeyMods::NONE,
                })
                .unwrap();
        }
        let found = screen(&mut backend);
        assert!(found.contains("new Ctrl+N"), "{found}");
        assert!(found.contains("dev@ssh://workbox"), "{found}");
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(
            picker.entries[picker.selected].remote_target,
            Some(url.clone())
        );
        backend.dispatch(Msg::SessionPickerCreateFromQuery).unwrap();
        assert_eq!(
            backend.state().rename_session.as_ref().unwrap().host_target,
            Some(url)
        );
    });
}
