use super::discovery::{merge_current_session_row, push_cached_known_remote_rows};
use super::*;
use crate::config::Config;
use crate::session::discovery::DiscoveredSession;
use crate::session::protocol::ClientInfo;
use crate::state::{NamingMode, SessionPickerState, SharedSessionState, State, ThemePreset};
use tui_lipan::prelude::*;

fn session_row(name: &str, host: Option<&str>) -> DiscoveredSession {
    DiscoveredSession {
        name: name.to_string(),
        origin: Default::default(),
        ephemeral: false,
        host: host.map(str::to_string),
        remote_target: host
            .map(|host| crate::session::remote::RemoteTarget::Alias(host.to_string())),
        status: crate::session::discovery::DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
        },
    }
}

#[test]
fn session_host_tabs_require_a_current_destination_not_cached_rows() {
    use crate::session::remote::RemoteTarget;
    use crate::state::{Attachment, HostProbe, SessionPickerTab};
    let mut state = State::new(Config::default(), Theme::default());
    *state.current_mut() = Attachment::new();
    let target = RemoteTarget::Alias("workbox".into());
    let mut row = session_row("dev", Some("workbox"));
    row.status = crate::session::discovery::DiscoveredSessionStatus::LastSeen { panes: 1 };
    state.session_picker = Some(SessionPickerState::new(vec![row]));
    let local_all = vec![SessionPickerTab::Host(None), SessionPickerTab::All];
    assert_eq!(session_picker_tabs(&state), local_all);

    // Known/configured hosts alone are not active destinations either.
    state.remote.hosts.seed(
        &state.config.remote,
        std::slice::from_ref(&target),
        &[],
        &[],
    );
    assert_eq!(session_picker_tabs(&state), local_all);
    let host_tabs = vec![
        SessionPickerTab::Host(None),
        SessionPickerTab::Host(Some(target.clone())),
        SessionPickerTab::All,
    ];
    state.remote.hosts.get_mut(&target).unwrap().probe = HostProbe::Reached;
    assert_eq!(session_picker_tabs(&state), host_tabs);
    state.remote.hosts.get_mut(&target).unwrap().probe = HostProbe::Idle;

    state.launcher_scope = Some(target.clone());
    assert_eq!(session_picker_tabs(&state), host_tabs);
    state.launcher_scope = None;
    state.current_mut().session_name = Some("dev".into());
    state.current_mut().remote_target = Some(target.clone());
    assert_eq!(session_picker_tabs(&state), host_tabs);
    state.current_mut().remote_target = None;
    let mut parked = Attachment::new();
    parked.remote_target = Some(target.clone());
    state.background.insert(1, parked);
    assert_eq!(session_picker_tabs(&state), host_tabs);
    state.background.clear();

    state.session_picker = Some(SessionPickerState::new(Vec::new()).on_tab(Some(target)));
    state.session_picker.as_mut().unwrap().tab = SessionPickerTab::Host(None);
    assert_eq!(
        session_picker_tabs(&state),
        host_tabs,
        "an explicitly opened destination survives browsing other tabs"
    );
}

#[test]
fn remembered_rows_of_hosts_without_a_tab_are_dropped_from_the_picker() {
    use crate::session::remote::RemoteTarget;
    let mut state = State::new(Config::default(), Theme::default());
    *state.current_mut() = crate::state::Attachment::new();
    let target = RemoteTarget::Alias("workbox".into());
    let mut remembered = session_row("dev", Some("workbox"));
    remembered.status = crate::session::discovery::DiscoveredSessionStatus::LastSeen { panes: 1 };
    let rows = vec![
        session_row("local", None),
        session_row("live", Some("other")),
        remembered,
    ];
    let names =
        |rows: &[DiscoveredSession]| rows.iter().map(|row| row.name.clone()).collect::<Vec<_>>();
    let mut kept = rows.clone();
    discovery::retain_tabbed_rows(&state, &mut kept);
    assert_eq!(names(&kept), ["local", "live"]);

    state.launcher_scope = Some(target);
    let mut kept = rows;
    discovery::retain_tabbed_rows(&state, &mut kept);
    assert_eq!(names(&kept), ["local", "live", "dev"]);
}

#[test]
fn opening_the_picker_leaves_a_cached_only_host_out_but_keeps_its_cache() {
    crate::test_support::on_large_stack(|| {
        crate::test_support::isolate_user_dirs();
        let _persist = crate::test_support::lock_persisted_state();
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.state_mut().command_link = None;
        let target = crate::session::remote::RemoteTarget::Alias("workbox".into());
        let state = backend.state_mut();
        state
            .config
            .remote
            .hosts
            .insert("workbox".into(), crate::config::RemoteHostConfig::default());
        state.remote.hosts.seed(&state.config.remote, &[], &[], &[]);
        crate::session::set_cached_host_sessions(
            &mut state.remote.session_cache,
            &target,
            vec![crate::session::CachedHostSession {
                name: "dev".into(),
                origin: Default::default(),
                ephemeral: false,
                panes: 1,
            }],
        );
        backend
            .update_level(crate::Msg::RunAction(
                crate::input::Action::OpenSessionPicker,
            ))
            .unwrap();
        let state = backend.state();
        let picker = state.session_picker.as_ref().unwrap();
        assert!(
            picker.entries.iter().all(|row| row.remote_target.is_none()),
            "a cached-only host is not listed in All"
        );
        assert_eq!(
            session_picker_tabs(state),
            [
                crate::state::SessionPickerTab::Host(None),
                crate::state::SessionPickerTab::All
            ]
        );
        assert!(
            crate::session::host_sessions_for(&state.remote.session_cache, &target)
                .is_some_and(|sessions| sessions.iter().any(|s| s.name == "dev")),
            "the remembered session is still cached for Remote hosts"
        );
    });
}

#[test]
fn session_picker_opening_preference_uses_context_or_all_without_changing_creation() {
    crate::test_support::on_large_stack(|| {
        crate::test_support::isolate_user_dirs();
        let _persist = crate::test_support::lock_persisted_state();
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        backend.state_mut().command_link = None;
        let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
        let other = crate::session::remote::RemoteTarget::Alias("other".into());
        for (attached, current_host, launcher_host, contextual_host) in [
            (false, None, None, None),
            (false, None, Some(workbox.clone()), Some(workbox.clone())),
            (true, None, Some(other.clone()), None),
            (true, Some(workbox.clone()), Some(other), Some(workbox)),
        ] {
            for preference in crate::config::SessionPickerOpenOn::all() {
                let state = backend.state_mut();
                *state.current_mut() = crate::state::Attachment::new();
                if attached {
                    state.current_mut().session_name = Some("dev".into());
                    state.current_mut().remote_target = current_host.clone();
                }
                state.launcher_scope = launcher_host.clone();
                state.config.session.picker_open_on = *preference;
                backend
                    .update_level(crate::Msg::RunAction(
                        crate::input::Action::OpenSessionPicker,
                    ))
                    .unwrap();
                let state = backend.state();
                let picker = state.session_picker.as_ref().unwrap();
                let expected = match preference {
                    crate::config::SessionPickerOpenOn::CurrentHost => {
                        crate::state::SessionPickerTab::Host(contextual_host.clone())
                    }
                    crate::config::SessionPickerOpenOn::All => crate::state::SessionPickerTab::All,
                };
                assert_eq!(picker.tab, expected);
                assert_eq!(session_picker_creation_target(state), contextual_host);
                assert_eq!(state.launcher_scope, launcher_host);
                backend
                    .update_level(crate::Msg::CloseSessionPicker)
                    .unwrap();
            }
        }
    });
}

/// Under `--remote` the discovery scan already returns the attached session, so merging the
/// current-session row must not add a second copy — otherwise the picker shows two
/// `name@host • current` entries. A same-name row on a *different* host is a real distinct
/// session and must stay.
#[test]
fn merge_current_session_row_dedupes_by_name_and_host() {
    let mut rows = vec![session_row("dev", Some("winvm"))];
    merge_current_session_row(&mut rows, session_row("dev", Some("winvm")));
    assert_eq!(
        rows.len(),
        1,
        "the attached session must not be listed twice"
    );

    // Same name, different host: a genuinely different session, kept.
    merge_current_session_row(&mut rows, session_row("dev", Some("other")));
    assert_eq!(rows.len(), 2);

    // Not present yet: added.
    let mut empty = Vec::new();
    merge_current_session_row(&mut empty, session_row("dev", Some("winvm")));
    assert_eq!(empty.len(), 1);
}

#[test]
fn picker_refresh_preserves_identity_and_clears_destructive_arms() {
    use crate::AppRoot;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let mut picker =
            SessionPickerState::new(vec![session_row("alpha", None), session_row("zulu", None)]);
        picker.selected = 1;
        picker.pending_kill = Some(1);
        backend.state_mut().session_picker = Some(picker);
        backend.state_mut().show_session_picker = true;
        let epoch = backend.state().session_picker_epoch;

        backend
            .update_level(crate::Msg::SessionsDiscovered {
                epoch,
                rows: vec![session_row("beta", None), session_row("zulu", None)],
                host_status: Vec::new(),
            })
            .expect("apply picker refresh");

        let picker = backend.state().session_picker.as_ref().expect("picker");
        assert_eq!(picker.entries[picker.selected].name, "zulu");
        assert!(picker.pending_kill.is_none());
        assert!(picker.pending_restart.is_none());
    });
}

/// A remote session this client is *attached* to is live by definition, and it is also in the host
/// cache — the cache is where it came from. The two rows merge by identity, so whichever is pushed
/// first wins, and pushing the cache first handed a session filling the screen a "last seen" label.
#[test]
fn an_attached_remote_session_outranks_its_own_cached_row() {
    use crate::AppRoot;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".to_string());
            state.current_mut().remote_host = Some("winvm".to_string());
            state.current_mut().remote_target = Some(target.clone());
            state.current_mut().session_attached = true;
            state.remote.hosts.seed(
                &crate::config::RemoteConfig::default(),
                std::slice::from_ref(&target),
                &[],
                &[],
            );
            crate::session::set_cached_host_sessions(
                &mut state.remote.session_cache,
                &target,
                vec![crate::session::CachedHostSession {
                    name: "dev".to_string(),
                    origin: Default::default(),
                    ephemeral: false,
                    panes: 9,
                }],
            );
            state.session_picker = Some(SessionPickerState::new(Vec::new()));
            state.show_session_picker = true;
        }
        let epoch = backend.state().session_picker_epoch;

        backend
            .update_level(crate::Msg::SessionsDiscovered {
                epoch,
                rows: Vec::new(),
                host_status: Vec::new(),
            })
            .expect("apply picker refresh");

        let picker = backend.state().session_picker.as_ref().expect("picker");
        let dev = picker
            .entries
            .iter()
            .find(|entry| entry.name == "dev")
            .expect("the attached session is listed");
        assert!(
            matches!(
                dev.status,
                crate::session::discovery::DiscoveredSessionStatus::Running { .. }
            ),
            "a session on screen is not a memory of one: {:?}",
            dev.status
        );
        assert_eq!(
            picker.entries.iter().filter(|e| e.name == "dev").count(),
            1,
            "the cached row merges into the live one rather than doubling it"
        );
    });
}

#[test]
fn cached_configured_hosts_are_available_without_a_probe() {
    let mut config = crate::config::RemoteConfig::default();
    config.hosts.insert(
        "winvm".to_string(),
        crate::config::RemoteHostConfig::default(),
    );
    let mut cache = crate::session::HostSessionCache::new();
    cache.insert(
        "winvm".to_string(),
        vec![crate::session::CachedHostSession {
            name: "dev".to_string(),
            origin: Default::default(),
            ephemeral: false,
            panes: 4,
        }],
    );
    let mut rows = vec![session_row("local", None)];

    let mut hosts = crate::state::HostRegistry::default();
    hosts.seed(&config, &[], &[], &[]);
    push_cached_known_remote_rows(&mut rows, &hosts, &cache, &[]);

    let remote = rows
        .iter()
        .find(|row| row.name == "dev")
        .expect("cached remote row");
    assert_eq!(remote.host.as_deref(), Some("winvm"));
    assert_eq!(
        remote.remote_target,
        Some(crate::session::remote::RemoteTarget::Alias(
            "winvm".to_string()
        ))
    );
    // Cached, so it must not be able to pass for a live session anywhere downstream.
    assert!(matches!(
        remote.status,
        crate::session::discovery::DiscoveredSessionStatus::LastSeen { panes: 4 }
    ));
}

#[test]
fn fresh_host_results_replace_cached_rows() {
    let mut config = crate::config::RemoteConfig::default();
    config.hosts.insert(
        "winvm".to_string(),
        crate::config::RemoteHostConfig::default(),
    );
    let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
    let mut cache = crate::session::HostSessionCache::new();
    cache.insert(
        "winvm".to_string(),
        vec![crate::session::CachedHostSession {
            name: "stale".to_string(),
            origin: Default::default(),
            ephemeral: false,
            panes: 2,
        }],
    );
    let mut rows = vec![session_row("live", Some("winvm"))];

    let mut hosts = crate::state::HostRegistry::default();
    hosts.seed(&config, &[], &[], &[]);
    push_cached_known_remote_rows(&mut rows, &hosts, &cache, std::slice::from_ref(&target));

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "live");
}

#[test]
fn cached_recent_and_attached_hosts_are_available_without_a_probe() {
    let recent = crate::session::remote::RemoteTarget::Alias("recent".into());
    let attached = crate::session::remote::RemoteTarget::Alias("attached".into());
    let mut hosts = crate::state::HostRegistry::default();
    hosts.seed(
        &crate::config::RemoteConfig::default(),
        &[],
        std::slice::from_ref(&recent),
        &[(attached.clone(), "attached".into())],
    );
    let mut cache = crate::session::HostSessionCache::new();
    for (target, name) in [(&recent, "recent-dev"), (&attached, "attached-dev")] {
        crate::session::set_cached_host_sessions(
            &mut cache,
            target,
            vec![crate::session::CachedHostSession {
                name: name.into(),
                origin: Default::default(),
                ephemeral: false,
                panes: 1,
            }],
        );
    }
    let mut rows = Vec::new();
    push_cached_known_remote_rows(&mut rows, &hosts, &cache, &[]);
    assert!(rows.iter().any(|row| row.name == "recent-dev"));
    assert!(rows.iter().any(|row| row.name == "attached-dev"));
}

fn ephemeral_state(client_id: u64, controller: u64, clients: Vec<ClientInfo>) -> State {
    let mut state = State::new(Config::default(), ThemePreset::Lipan.theme());
    state.current_mut().session_name = Some("eph-test".to_string());
    state.current_mut().session_attached = true;
    let mut shared = SharedSessionState::new(client_id);
    shared.controller = Some(controller);
    shared.clients = clients;
    state.current_mut().shared = Some(shared);
    state
}

#[test]
fn follower_request_control_asks_the_controller_without_stealing() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.current_mut().session_attached = true;
            state.current_mut().session_client = Some(client);
            // A follower: client 2 holds the lease.
            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(2);
            shared.clients = vec![
                ClientInfo {
                    id: 1,
                    label: "me".into(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                },
                ClientInfo {
                    id: 2,
                    label: "them".into(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                },
            ];
            state.current_mut().shared = Some(shared);
        }
        backend.render();
        backend
            .dispatch(Msg::RunAction(Action::RequestControl))
            .expect("dispatch request-control");

        let sent: Vec<ClientOutbound> = rx.try_iter().collect();
        assert!(
            sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::RequestControl)
            )),
            "a follower must ask for control, got {sent:?}"
        );
        assert!(
            !sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::GrantControl { .. })
            )),
            "requesting must never steal the lease"
        );
    });
}

/// Set up a controller on a session shared with one writable client that wants the lease.
fn shared_controller_backend() -> tui_lipan::TestBackend<crate::AppRoot> {
    use crate::AppRoot;
    use crate::session::client::SessionClient;
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 90,
        h: 28,
    });
    let (client, _rx) = SessionClient::test_channel();
    let state = backend.state_mut();
    state.current_mut().session_name = Some("dev".into());
    state.current_mut().session_attached = true;
    state.current_mut().session_client = Some(client);
    let mut shared = SharedSessionState::new(1);
    shared.controller = Some(1);
    shared.clients = vec![
        ClientInfo {
            id: 1,
            label: "me".into(),
            read_only: false,
            requesting_control: false,
            parked: false,
        },
        ClientInfo {
            id: 2,
            label: "laptop".into(),
            read_only: false,
            requesting_control: true,
            parked: false,
        },
    ];
    state.current_mut().shared = Some(shared);
    backend
}

/// The dialog is rows and chrome, never prose: this client's identity and role ride the top
/// border as a right header, the other clients are rows with compact markers, and the keys that
/// currently apply are footer pills. Nothing states a fact in a sentence.
#[test]
fn collaborators_dialog_is_rows_and_chrome_with_no_prose_line() {
    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

        backend.render();
        let rendered = backend.capture_frame().to_fixed_grid();
        // Title and self-context share the top border, so neither costs a content row.
        assert!(
            rendered.contains("Collaborators"),
            "expected the title on the border: {rendered}"
        );
        assert!(
            rendered.contains("me #1 · ctrl"),
            "expected the self tag as a right header: {rendered}"
        );
        assert!(
            !rendered.contains("You:"),
            "the self context must not be a prose line: {rendered}"
        );
        assert_eq!(rendered.matches("me #1").count(), 1, "{rendered}");
        assert!(rendered.contains("Search other clients"), "{rendered}");
        assert!(rendered.contains("laptop #2"), "{rendered}");
        assert!(rendered.contains("wants ctrl"), "{rendered}");
        // Every key that applies is advertised, and each is a Ctrl chord or Enter, because
        // the query input owns focus and a bare letter has to reach the filter.
        assert!(rendered.contains("grant control Enter"), "{rendered}");
        assert!(rendered.contains("decline Ctrl+D"), "{rendered}");
        assert!(rendered.contains("kick Ctrl+K"), "{rendered}");
    });
}

/// Typing filters the roster instead of triggering actions: the letters that used to navigate
/// or act (`j`, `k`, `g`, `d`, `x`) must reach the query input now that it owns focus.
#[test]
fn plain_letters_reach_the_filter_instead_of_acting() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        let (client, rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
        backend.render();

        for letter in ['j', 'k', 'g', 'd', 'x'] {
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Char(letter),
                    mods: KeyMods::NONE,
                })
                .expect("send letter");
        }

        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            !sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(
                    ClientMessage::GrantControl { .. }
                        | ClientMessage::DeclineControl { .. }
                        | ClientMessage::EvictClient { .. }
                )
            )),
            "typing must not act on a client, got {sent:?}"
        );
        assert!(
            backend
                .state()
                .collaboration
                .as_ref()
                .is_some_and(|collaboration| collaboration.pending_kick.is_none()),
            "typing must not arm a removal"
        );
        // The letters landed in the filter, which is the whole point of freeing them.
        let rendered = backend.capture_frame().to_fixed_grid();
        assert!(rendered.contains("jkgdx"), "{rendered}");
        assert!(!rendered.contains("laptop #2"), "{rendered}");
    });
}

/// An empty list means two different things, and the message must not claim the wrong one: a
/// query that matched nobody is not the same as a session nobody else is on.
#[test]
fn an_empty_list_says_whether_it_is_the_filter_or_the_roster() {
    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
        backend
            .dispatch(crate::Msg::CollaborationQueryChanged("zzz".to_string()))
            .expect("filter to nothing");
        backend.render();
        let filtered = backend.capture_frame().to_fixed_grid();
        assert!(
            filtered.contains("No client matches `zzz`"),
            "a filtered-out roster must name the query: {filtered}"
        );
        assert!(
            !filtered.contains("No other clients"),
            "clients are attached, so claiming otherwise is false: {filtered}"
        );

        // The same dialog with the roster genuinely empty says so, query or not.
        if let Some(shared) = backend.state_mut().current_mut().shared.as_mut() {
            shared.clients.retain(|client| client.id == 1);
        }
        backend.render();
        let empty = backend.capture_frame().to_fixed_grid();
        assert!(empty.contains("No other clients"), "{empty}");
        assert!(!empty.contains("No client matches"), "{empty}");
    });
}

/// A query that hides every client must leave nothing to act on: the footer stops advertising
/// keys, and `ctrl+k` cannot reach a row scrolled out of sight by the filter.
#[test]
fn a_filter_that_hides_everyone_disarms_the_dialog() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        let (client, rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());
        backend.render();

        // Positive control: unfiltered, the chord reaches the interceptor and arms the row.
        // Without this the negative below would pass even if no key arrived at all.
        backend
            .send_key(KeyEvent {
                code: KeyCode::Char('k'),
                mods: KeyMods::CTRL,
            })
            .expect("send ctrl+k");
        assert_eq!(
            backend
                .state()
                .collaboration
                .as_ref()
                .and_then(|collaboration| collaboration.pending_kick),
            Some(2),
            "ctrl+k must arm the highlighted client when it is visible"
        );

        backend
            .dispatch(crate::Msg::CollaborationQueryChanged("zzz".to_string()))
            .expect("filter to nothing");
        backend.render();
        let rendered = backend.capture_frame().to_fixed_grid();
        assert!(!rendered.contains("kick"), "{rendered}");
        assert!(!rendered.contains("grant control"), "{rendered}");

        backend
            .send_key(KeyEvent {
                code: KeyCode::Char('k'),
                mods: KeyMods::CTRL,
            })
            .expect("send ctrl+k");
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            !sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::EvictClient { .. })
            )),
            "a hidden client must not be removable, got {sent:?}"
        );
    });
}

/// Removing a client is destructive to somebody else's attachment, so the first press only arms
/// the row and nothing goes on the wire until the second one.
#[test]
fn kicking_a_collaborator_takes_two_presses() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        // `test_channel` speaks this build's maximum protocol, which is what gates evicting.
        let (client, rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

        backend
            .dispatch(crate::Msg::CollaborationKick(1))
            .expect("arm the removal");
        assert_eq!(
            backend
                .state()
                .collaboration
                .as_ref()
                .and_then(|collaboration| collaboration.pending_kick),
            Some(2),
            "arming is held by client id, not roster position"
        );
        let armed_traffic: Vec<_> = rx.try_iter().collect();
        assert!(
            !armed_traffic.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::EvictClient { .. })
            )),
            "arming must not remove anyone yet, got {armed_traffic:?}"
        );

        backend
            .dispatch(crate::Msg::CollaborationKick(1))
            .expect("confirm the removal");
        let sent: Vec<_> = rx.try_iter().collect();
        assert!(
            sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::EvictClient { target: 2 })
            )),
            "expected an evict for client 2, got {sent:?}"
        );
        assert!(
            backend
                .state()
                .collaboration
                .as_ref()
                .is_some_and(|collaboration| collaboration.pending_kick.is_none())
        );
    });
}

/// The arming runs on the shared confirmation clock, so a kick left half-pressed lapses like
/// every other destructive gesture rather than waiting indefinitely for a second key.
#[test]
fn an_unconfirmed_kick_lapses_on_the_shared_confirm_window() {
    use crate::session::client::SessionClient;

    crate::test_support::on_large_stack(|| {
        let mut backend = shared_controller_backend();
        let (client, _rx) = SessionClient::test_channel();
        backend.state_mut().current_mut().session_client = Some(client);
        backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

        backend
            .dispatch(crate::Msg::CollaborationKick(1))
            .expect("arm the removal");
        let armed_epoch = backend.state().confirm_epoch;
        assert!(
            backend
                .state()
                .collaboration
                .as_ref()
                .is_some_and(|collaboration| collaboration.pending_kick.is_some()),
            "arming must register with the shared clock"
        );

        backend
            .dispatch(crate::Msg::ConfirmationExpired(armed_epoch))
            .expect("the window lapses");
        assert!(
            backend
                .state()
                .collaboration
                .as_ref()
                .is_some_and(|collaboration| collaboration.pending_kick.is_none()),
            "an unconfirmed kick must disarm itself"
        );
    });
}

#[test]
fn occupied_session_prompt_keeps_context_in_the_title() {
    use crate::AppRoot;
    use crate::state::FollowPromptState;
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 72,
            h: 20,
        });
        backend.state_mut().follow_prompt = Some(FollowPromptState {
            session: "test".into(),
            controller_label: "razuer".into(),
            allow_takeover: true,
            selected: 0,
        });

        backend.render();
        let rendered = backend.capture_frame().to_fixed_grid();
        assert!(rendered.contains("`test` in use by razuer"), "{rendered}");
        assert!(!rendered.contains("is being driven"), "{rendered}");
        assert!(rendered.contains("no layout control"), "{rendered}");
        assert!(rendered.contains("control moves to you"), "{rendered}");
        assert!(rendered.contains("go back"), "{rendered}");
    });
}

#[test]
fn cancelling_occupied_attach_does_not_retain_it_as_offline() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::{Attachment, ConnectionState, FollowPromptState};
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (target_client, target_rx) = SessionClient::test_channel();
        let (survivor_client, _survivor_rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.runtime_epoch = 10;
            state.current_mut().epoch = 10;
            state.current_mut().session_name = Some("occupied".into());
            state.current_mut().session_attached = true;
            state.current_mut().connection = ConnectionState::Connected;
            state.current_mut().session_client = Some(target_client);
            let mut shared = SharedSessionState::new(2);
            shared.controller = Some(1);
            shared.clients = vec![
                ClientInfo {
                    id: 1,
                    label: "desktop".into(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                },
                ClientInfo {
                    id: 2,
                    label: "laptop".into(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                },
            ];
            state.current_mut().shared = Some(shared);

            let mut survivor = Attachment::new();
            survivor.epoch = 5;
            survivor.parked_seq = 1;
            survivor.session_name = Some("previous".into());
            survivor.session_attached = true;
            survivor.connection = ConnectionState::Connected;
            survivor.session_client = Some(survivor_client);
            let mut survivor_shared = SharedSessionState::new(1);
            survivor_shared.controller = Some(1);
            survivor.shared = Some(survivor_shared);
            state.background.insert(5, survivor);
            state.follow_prompt = Some(FollowPromptState {
                session: "occupied".into(),
                controller_label: "desktop".into(),
                allow_takeover: false,
                selected: 2,
            });
        }

        backend
            .dispatch(Msg::FollowPromptChoose(2))
            .expect("cancel occupied attach");

        let state = backend.state();
        assert!(
            state.is_launcher(),
            "cancelling leaves the foreground sessionless rather than auto-attaching"
        );
        assert!(
            state.show_session_picker,
            "the parked previous session remains as a choice"
        );
        assert!(!state.background.contains_key(&10));
        assert!(state.attachment_by_identity("occupied", None).is_none());
        assert!(
            state
                .background
                .values()
                .any(|attachment| { attachment.session_name.as_deref() == Some("previous") }),
            "previous stays parked for an explicit picker choice"
        );
        assert!(
            target_rx
                .try_iter()
                .any(|message| matches!(message, ClientOutbound::Control(ClientMessage::Detach)))
        );
    });
}

#[test]
fn killing_the_last_attached_session_stays_sessionless_without_auto_attach() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::SessionClient;
    use crate::state::ConnectionState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let _persist = crate::test_support::lock_persisted_state();
        crate::session::reset_host_session_cache();

        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.config.confirm.kill_session = false;
            state.current_mut().session_name = Some("solo".into());
            state.current_mut().session_attached = true;
            state.current_mut().connection = ConnectionState::Connected;
            state.current_mut().session_client = Some(client);
            state.current_mut().pending_session_attach = None;
            state.background.clear();
            state.remote.session_cache.clear();
            state.show_session_picker = false;
            state.session_picker = None;
        }

        backend
            .dispatch(Msg::RunAction(Action::KillSession))
            .expect("kill last session");

        let state = backend.state();
        assert!(state.is_launcher());
        assert!(state.current().pending_session_attach.is_none());
        // Seeded with an empty discovery set and an empty host-session cache. Do not re-read
        // `has_session_candidates()` here: that predicate looks at process-wide files another
        // test may write after this kill has already decided.
        assert!(!state.show_session_picker);
        assert!(state.session_picker.is_none());
    });
}

#[test]
fn starting_a_shell_from_the_picker_attaches_the_ephemeral_with_the_launch_seed() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionPickerState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            // The startup picker parks the panes the launch prepared and leaves the
            // foreground empty, which is the launcher the scratch key starts from.
            let mut seed = crate::state::fresh_default_attachment(&state.config);
            seed.workspaces[0].panes[0].identity.cwd = Some("/seeded".into());
            *state.current_mut() = crate::state::Attachment::new();
            state.launcher_seed = Some(seed);
            state.show_session_picker = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
        }
        assert!(backend.state().is_launcher());

        backend
            .dispatch(Msg::SessionPickerEphemeral)
            .expect("start a shell from the picker");

        let state = backend.state();
        assert!(!state.show_session_picker && state.session_picker.is_none());
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("attaching the ephemeral session");
        assert_eq!(pending.request.name, crate::state::ephemeral_session_name());
        assert!(
            state.launcher_seed.is_none(),
            "the parked launch panes are consumed, not left for a second start"
        );
        assert_eq!(
            state.current().workspaces[0].panes[0]
                .identity
                .cwd
                .as_deref(),
            Some("/seeded"),
            "the shell starts with the layout the launch intended"
        );
    });
}

#[test]
fn creating_a_session_with_an_existing_name_keeps_the_prompt_and_shows_an_inline_error() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use crate::state::{ConnectionState, SessionRenameState};
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            // Current attachment owns `dev`, so create must treat that name as taken.
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.current_mut().connection = ConnectionState::Connected;
            state.current_mut().session_client = Some(client);
            state.current_mut().pending_session_attach = None;
            state.overlay_return = Some(crate::state::OverlayOrigin::SessionPicker {
                query: String::new(),
                selected_session: None,
                tab: crate::state::SessionPickerTab::Host(None),
                opened_host: None,
                browse_selected: None,
            });
            state.rename_session = Some(SessionRenameState::new("dev", NamingMode::CreateSession));
        }

        backend
            .dispatch(Msg::SubmitRenameSession)
            .expect("submit colliding create");

        let state = backend.state();
        let rename = state
            .rename_session
            .as_ref()
            .expect("create prompt must stay open");
        assert_eq!(
            rename.error.as_deref(),
            Some("Session `dev` is already running")
        );
        assert_eq!(rename.input.text(), "dev");
        assert!(
            state.current().pending_session_attach.is_none(),
            "a rejected create must not start an attach"
        );
        assert_eq!(
            state.current().session_name.as_deref(),
            Some("dev"),
            "the active session must stay put"
        );
        assert!(
            state.overlay_return.is_some(),
            "parent picker origin must survive a rejected create"
        );
    });
}

#[test]
fn create_session_starts_fresh_instead_of_carrying_current_panes() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionRenameState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        // A profile name unlikely to exist on disk, so resolution must fall through to a
        // fresh empty session rather than a profile seed.
        let name = format!("create-fresh-{}", std::process::id());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("eph-test".to_string());
            state.current_mut().session_attached = true;
            state.current_mut().engaged = true;
            state.current_mut().pending_session_attach = None;
            state.sidebar.command_epoch = 7;
            state.sidebar.config_epoch = 11;
            // Client-global chrome that must survive a create: an open sidebar on a chosen
            // tab, and live workbar command scheduling state.
            state.sidebar.shown = [true, false];
            state.sidebar.panels[0].active_tab = Some(crate::config::SidebarTabId::new("sessions"));
            state.workbar.command_epoch = 3;
            state
                .workbar
                .command_in_flight
                .insert("date".to_string(), 3);
            // Simulate a profile-seeded session: the current pane carries a command.
            state.current_mut().workspaces[0].panes[0].identity.launch =
                Some(crate::pane::launch::PaneLaunch::shell("nvim"));
            state.rename_session = Some(SessionRenameState::new(&name, NamingMode::CreateSession));
        }
        backend.render();
        // `update_level`, not `dispatch`: every assertion below is about what the create
        // installs synchronously, and no server is ever going to answer for this name.
        // `dispatch` drains until idle, so the create thread's fast failure could land in
        // the same pump and tear the pending attach back down before the test reads it.
        backend
            .update_level(Msg::SubmitRenameSession)
            .expect("dispatch create session");

        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("create queues an attach");
        assert_eq!(pending.request.name, name);
        assert_eq!(pending.request.intent, crate::state::AttachIntent::Plain);
        // Creating from an attached (ephemeral) session parks it rather than detaching, so
        // there is no "left" session named in the toast, and the parked id is recorded so a
        // failed attach can restore it. The parked session is retained in the background.
        assert_eq!(pending.left, None);
        let parked_epoch = pending.parked_epoch.expect("current session was parked");
        assert!(state.background.contains_key(&parked_epoch));
        assert_eq!(
            state.background[&parked_epoch].session_name.as_deref(),
            Some("eph-test")
        );
        // The new session must not inherit the current layout: the installed attachment is a
        // fresh single-pane default with no launch command to respawn.
        assert_eq!(state.current().workspaces[0].panes.len(), 1);
        assert_eq!(state.current().workspaces[0].panes[0].identity.launch, None);
        // Client-global state is not per-session, so installing a fresh attachment leaves it
        // untouched: the config epoch doesn't churn, the sidebar stays open on its tab, and
        // workbar scheduling stays live. This is the whole point of installing an attachment
        // instead of rebuilding State.
        //
        // The sidebar command epoch does advance by one, because the new attachment's focused
        // pane is somewhere else and a command tab describes the project in front of you. The
        // cached rows stay on screen until the new ones land, so this re-lists without a
        // flicker rather than blanking the tab.
        assert_eq!(state.sidebar.command_epoch, 8);
        assert_eq!(state.sidebar.config_epoch, 11);
        assert!(state.sidebar_shown());
        assert_eq!(
            state.sidebar.active_tab(),
            Some(&crate::config::SidebarTabId::new("sessions"))
        );
        assert_eq!(state.workbar.command_epoch, 3);
        assert_eq!(state.workbar.command_in_flight.get("date"), Some(&3));
    });
}

#[test]
fn controller_grant_control_key_grants_to_the_earliest_requester() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.current_mut().session_attached = true;
            state.current_mut().session_client = Some(client);
            // We (client 1) are the controller; clients 2 and 3 both want control.
            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(1);
            let requester = |id| ClientInfo {
                id,
                label: format!("c{id}"),
                read_only: false,
                requesting_control: true,
                parked: false,
            };
            shared.clients = vec![
                ClientInfo {
                    id: 1,
                    label: "me".into(),
                    read_only: false,
                    requesting_control: false,
                    parked: false,
                },
                requester(3),
                requester(2),
            ];
            state.current_mut().shared = Some(shared);
        }
        backend.render();
        backend
            .dispatch(Msg::RunAction(Action::GrantControl))
            .expect("dispatch grant-control");

        let sent: Vec<ClientOutbound> = rx.try_iter().collect();
        // The earliest requester (smallest id = 2) is granted, not the roster's first entry.
        assert!(
            sent.iter().any(|message| matches!(
                message,
                ClientOutbound::Control(ClientMessage::GrantControl { to: 2 })
            )),
            "expected a grant to client 2, got {sent:?}"
        );
    });
}

#[test]
fn only_solo_ephemeral_controller_may_shutdown_on_release() {
    let client = |id| ClientInfo {
        id,
        label: format!("client-{id}"),
        read_only: false,
        requesting_control: false,
        parked: false,
    };
    assert!(may_shutdown_ephemeral(&ephemeral_state(
        1,
        1,
        vec![client(1)]
    )));
    assert!(!may_shutdown_ephemeral(&ephemeral_state(
        2,
        1,
        vec![client(1), client(2)]
    )));
    assert!(!may_shutdown_ephemeral(&ephemeral_state(
        1,
        1,
        vec![client(1), client(2)]
    )));
}

/// Leaving a session for another one: whether the one being left is kept alive in the
/// background. Driven through the create-session flow, the same path a switch takes.
fn background_after_leaving_ephemeral(engaged: bool) -> bool {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use crate::state::{NamingMode, SessionRenameState};
    use tui_lipan::TestBackend;

    let (tx, rx) = std::sync::mpsc::channel();
    crate::test_support::on_large_stack(move || {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _outbound) = SessionClient::test_channel();
        let name = format!("leave-target-{}", std::process::id());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("eph-startup".to_string());
            state.current_mut().session_attached = true;
            state.current_mut().session_client = Some(client);
            // What a bare launch produces: an ephemeral the client picked the name for.
            state.current_mut().auto_created = true;
            state.current_mut().engaged = engaged;
            state.current_mut().pending_session_attach = None;
            let mut shared = SharedSessionState::new(1);
            shared.controller = Some(1);
            shared.clients = vec![ClientInfo {
                id: 1,
                label: "me".into(),
                read_only: false,
                requesting_control: false,
                parked: false,
            }];
            state.current_mut().shared = Some(shared);
            state.rename_session = Some(SessionRenameState::new(&name, NamingMode::CreateSession));
        }
        backend.render();
        // Synchronous outcome only - see `create_session_starts_fresh…`: draining until
        // idle lets the create thread's failure restore the session this asserts was
        // parked.
        backend
            .update_level(Msg::SubmitRenameSession)
            .expect("dispatch create session");
        tx.send(!backend.state().background.is_empty())
            .expect("report result");
    });
    rx.recv().expect("test result")
}

/// The startup ephemeral is the session nobody asked for. Switching away from an untouched one
/// must remove it, not leave it running where it later shows up as a session to confirm away.
#[test]
fn switching_away_discards_an_untouched_startup_ephemeral() {
    assert!(!background_after_leaving_ephemeral(false));
}

/// The same ephemeral, once worked in, is real work: switching away parks it so it can be
/// switched back to.
#[test]
fn switching_away_parks_a_used_ephemeral() {
    assert!(background_after_leaving_ephemeral(true));
}

/// Attaching to a session that is already retained must retire the Profiles overlay the same way
/// a launch does — otherwise Enter on a running profile leaves the picker covering the session
/// that just came to the foreground.
#[test]
fn attaching_to_parked_session_closes_profile_picker() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use crate::state::{Attachment, ConnectionState, ProfilePickerState};
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (current_client, _current_rx) = SessionClient::test_channel();
        let (parked_client, _parked_rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.runtime_epoch = 1;
            state.current_mut().epoch = 1;
            state.current_mut().session_name = Some("other".into());
            state.current_mut().session_attached = true;
            state.current_mut().connection = ConnectionState::Connected;
            state.current_mut().session_client = Some(current_client);

            let mut parked = Attachment::new();
            parked.epoch = 2;
            parked.parked_seq = 1;
            parked.session_name = Some("dev".into());
            parked.session_attached = true;
            parked.connection = ConnectionState::Connected;
            parked.session_client = Some(parked_client);
            state.background.insert(2, parked);

            state.show_profile_picker = true;
            state.profile_picker = Some(ProfilePickerState::new(Vec::new()));
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
            state.show_session_picker = true;
        }
        let revision = backend.state().session_view_revision;

        backend
            .dispatch(Msg::SessionPickerActivate(0))
            .expect("attach to parked session");

        assert_eq!(
            backend.state().current().session_name.as_deref(),
            Some("dev")
        );
        assert_eq!(
            backend.state().session_view_revision,
            revision + 1,
            "a parked session coming forward is a new view to reveal"
        );
        assert!(!backend.state().show_profile_picker);
        assert!(backend.state().profile_picker.is_none());
        assert!(!backend.state().show_session_picker);
        assert!(backend.state().session_picker.is_none());
    });
}

/// A cold attach (session running, not retained here) must dismiss Profiles as soon as the
/// switch starts — not wait for `SessionAttached`, which left the overlay up over Connecting.
#[test]
fn cold_attach_closes_profile_picker_before_connect() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use crate::state::{ConnectionState, ProfilePickerState};
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.runtime_epoch = 1;
            state.current_mut().epoch = 1;
            state.current_mut().session_name = Some("other".into());
            state.current_mut().session_attached = true;
            state.current_mut().connection = ConnectionState::Connected;
            state.current_mut().session_client = Some(client);
            state.show_profile_picker = true;
            state.profile_picker = Some(ProfilePickerState::new(Vec::new()));
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
            state.show_session_picker = true;
        }

        // `update_level`, not `dispatch`: the assertions below are about the state the
        // switch leaves behind *before* it connects, and "dev" is not a session that
        // exists here. `dispatch` drains until idle, so the attach thread's fast failure
        // could land in the same pump and clear `pending_session_attach` out from under
        // the test - which it did, on roughly one run in five.
        backend
            .update_level(Msg::SessionPickerActivate(0))
            .expect("start cold attach");

        assert!(!backend.state().show_profile_picker);
        assert!(backend.state().profile_picker.is_none());
        assert!(
            backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .is_some_and(|pending| pending.request.name == "dev")
        );
    });
}

#[test]
fn activating_restorable_session_autostarts_its_server() {
    crate::test_support::on_large_stack(|| {
        use crate::AppRoot;
        use crate::Msg;
        use tui_lipan::TestBackend;

        let mut backend = TestBackend::new(AppRoot::default());
        let mut row = session_row("saved", None);
        row.status = crate::session::discovery::DiscoveredSessionStatus::Restorable;
        backend.state_mut().session_picker = Some(SessionPickerState::new(vec![row]));
        backend.state_mut().show_session_picker = true;

        backend
            .update_level(Msg::SessionPickerActivate(0))
            .expect("start snapshot restore");

        let pending = backend
            .state()
            .current()
            .pending_session_attach
            .as_ref()
            .expect("snapshot attach");
        assert_eq!(pending.request.name, "saved");
        assert_eq!(pending.request.mode, crate::state::AttachMode::JoinOrStart);
    });
}

#[test]
fn restart_is_inert_for_a_restorable_session() {
    crate::test_support::on_large_stack(|| {
        use crate::AppRoot;
        use crate::Msg;
        use tui_lipan::TestBackend;

        let mut backend = TestBackend::new(AppRoot::default());
        let mut row = session_row("saved", None);
        row.status = crate::session::discovery::DiscoveredSessionStatus::Restorable;
        backend.state_mut().session_picker = Some(SessionPickerState::new(vec![row]));
        backend.state_mut().show_session_picker = true;

        backend
            .dispatch(Msg::SessionPickerRestartSelected)
            .expect("restart a restorable row");

        let picker = backend.state().session_picker.as_ref().expect("picker");
        assert!(
            picker.pending_restart.is_none(),
            "restart must not arm against a snapshot"
        );
        assert!(
            backend.state().show_session_picker,
            "restart must not consume the picker the way a restore would"
        );
        assert!(
            backend
                .state()
                .current()
                .pending_session_attach
                .as_ref()
                .is_none_or(|pending| pending.request.name != "saved"),
            "restart must not restore the snapshot"
        );
    });
}

#[test]
fn activating_the_current_session_is_inert() {
    crate::test_support::on_large_stack(|| {
        use crate::AppRoot;
        use crate::Msg;
        use tui_lipan::TestBackend;

        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.current_mut().pending_session_attach = None;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
            state.show_session_picker = true;
        }

        backend
            .update_level(Msg::SessionPickerActivate(0))
            .expect("activate the current session");

        assert!(
            backend.state().show_session_picker,
            "Enter on the current session must leave the picker open"
        );
        assert!(
            backend.state().replaceable_toasts.is_empty(),
            "already being there must not toast"
        );
        assert!(
            backend.state().current().pending_session_attach.is_none(),
            "must not start a new attach"
        );
    });
}

#[test]
fn disconnecting_the_current_session_is_inert() {
    crate::test_support::on_large_stack(|| {
        use crate::AppRoot;
        use crate::Msg;
        use tui_lipan::TestBackend;

        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("dev".into());
            state.current_mut().session_attached = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
            state.show_session_picker = true;
        }

        backend
            .dispatch(Msg::SessionPickerDisconnectAttachment)
            .expect("disconnect the current session");

        assert!(backend.state().show_session_picker);
        assert!(
            backend.state().replaceable_toasts.is_empty(),
            "a chord the footer omitted must not toast"
        );
    });
}

#[test]
fn disconnecting_a_session_we_do_not_hold_is_inert() {
    crate::test_support::on_large_stack(|| {
        use crate::AppRoot;
        use crate::Msg;
        use tui_lipan::TestBackend;

        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("here".into());
            state.current_mut().session_attached = true;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("there", None)]));
            state.show_session_picker = true;
        }

        backend
            .dispatch(Msg::SessionPickerDisconnectAttachment)
            .expect("disconnect a session we do not hold");

        assert!(backend.state().show_session_picker);
        assert!(
            backend.state().replaceable_toasts.is_empty(),
            "not-connected must not toast when disconnect is not offered"
        );
    });
}

/// The rule the whole model rests on: an action operates on the scope its surface *shows*. Global
/// Sessions shows every host at once, so its `Ctrl+T` starts a local shell — even with a remote
/// session filling the screen behind the overlay. Inheriting `workbox` from whatever happened to be
/// attached would make one key mean two things on one screen.
#[test]
fn the_global_pickers_scratch_key_stays_local_behind_a_remote_session() {
    use crate::AppRoot;
    use crate::Msg;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some("backend".into());
            state.current_mut().session_attached = true;
            state.current_mut().remote_host = Some("workbox".into());
            state.current_mut().remote_target = Some(workbox);
            state.session_picker = Some(SessionPickerState::new(vec![session_row(
                "backend",
                Some("workbox"),
            )]));
            state.show_session_picker = true;
        }

        backend
            .update_level(Msg::SessionPickerEphemeral)
            .expect("start the scratch session from the global picker");

        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("an attach is in flight");
        assert_eq!(pending.request.name, crate::state::ephemeral_session_name());
        assert!(
            pending.request.remote.is_none(),
            "the global picker's scratch key must not inherit the attached session's host"
        );
        assert!(backend.state().current().remote_target.is_none());
    });
}

/// A scratch session started from inside another session opens with a shell, the same as one
/// started from the launcher or a fresh named session. The session it leaves takes its panes with
/// it, and the new server is seeded from the panes installed in its place, so an empty install
/// would open an empty session.
#[test]
fn a_scratch_session_started_from_an_attached_session_opens_with_a_pane() {
    use crate::AppRoot;
    use crate::Msg;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let (client, _outbound) = crate::session::client::SessionClient::test_channel();
        {
            let state = backend.state_mut();
            let mut dev = crate::state::fresh_default_attachment(&state.config);
            dev.session_name = Some("dev".into());
            dev.session_attached = true;
            dev.session_client = Some(client);
            *state.current_mut() = dev;
            state.session_picker = Some(SessionPickerState::new(vec![session_row("dev", None)]));
            state.show_session_picker = true;
        }

        backend
            .update_level(Msg::SessionPickerEphemeral)
            .expect("start the scratch session from inside `dev`");

        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("an attach is in flight");
        assert_eq!(pending.request.name, crate::state::ephemeral_session_name());
        assert_eq!(
            state
                .current()
                .workspaces
                .iter()
                .map(|workspace| workspace.panes.len())
                .sum::<usize>(),
            1,
            "the new scratch session is seeded with one pane"
        );
    });
}

/// A remote scratch session on screen is a different session from the local one, so the key that
/// reaches the local one must still act rather than reading "you are already there".
#[test]
fn a_remote_scratch_session_does_not_answer_for_the_local_one() {
    use crate::AppRoot;
    use crate::Msg;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            state.current_mut().session_name = Some(crate::state::remote_ephemeral_session_name());
            state.current_mut().session_attached = true;
            state.current_mut().remote_host = Some("workbox".into());
            state.current_mut().remote_target = Some(crate::session::remote::RemoteTarget::Alias(
                "workbox".into(),
            ));
            state.session_picker = Some(SessionPickerState::new(Vec::new()));
            state.show_session_picker = true;
        }

        backend
            .dispatch(Msg::SessionPickerEphemeral)
            .expect("reach the local scratch session");

        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("an attach is in flight rather than a silent close");
        assert_eq!(pending.request.name, crate::state::ephemeral_session_name());
        assert!(pending.request.remote.is_none());
    });
}

/// The launcher, unlike the global picker, *does* name a machine — that is the whole content of its
/// `REMOTE · <host>` badge — so its one offer starts the shell there.
#[test]
fn a_scoped_launcher_starts_its_shell_on_the_host_it_names() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            *state.current_mut() = crate::state::Attachment::new();
            state.launcher_scope = Some(crate::session::remote::RemoteTarget::Alias(
                "workbox".into(),
            ));
        }
        assert!(backend.state().is_launcher());

        backend
            .update_level(Msg::RunAction(Action::Spawn))
            .expect("start the launcher's shell");

        let state = backend.state();
        let pending = state
            .current()
            .pending_session_attach
            .as_ref()
            .expect("an attach is in flight");
        assert_eq!(
            pending.request.name,
            crate::state::remote_ephemeral_session_name()
        );
        assert_eq!(pending.request.remote_label().as_deref(), Some("workbox"));
        assert_eq!(
            state
                .current()
                .remote_target
                .as_ref()
                .map(|target| target.display_label()),
            Some("workbox".to_string()),
        );
    });
}

/// A launcher scope is not a connection, and is only a scope while the foreground is empty. With a
/// session up, the field is a record of where the user is working — reading it as a scope would let
/// a host browsed earlier decide where a later kill lands.
#[test]
fn a_launcher_scope_is_only_read_while_the_foreground_is_empty() {
    let mut state = State::new(Config::default(), ThemePreset::Lipan.theme());
    // A launch seeds an initial pane; the launcher is what is left once nothing is in front.
    *state.current_mut() = crate::state::Attachment::new();
    state.launcher_scope = Some(crate::session::remote::RemoteTarget::Alias(
        "workbox".into(),
    ));
    assert!(state.is_launcher());
    assert!(state.active_launcher_scope().is_some());

    state.current_mut().session_name = Some("dev".into());
    assert!(!state.is_launcher());
    assert!(state.active_launcher_scope().is_none());
}

/// `Ctrl+X` walks away from a host; it does not clean it out. A named remote session keeps running
/// whichever client started it — killing it is `Ctrl+K`'s job alone. The one thing torn down is a
/// scratch session this client made on the user's behalf and that they never worked in: nothing can
/// reattach to it by name, so leaving it would only litter a machine we are done with.
#[test]
fn disconnecting_a_host_keeps_named_sessions_and_drops_only_the_disposable_scratch_one() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::SessionPickerState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
        let (named_client, named_rx) = SessionClient::test_channel();
        let (scratch_client, scratch_rx) = SessionClient::test_channel();
        {
            let state = backend.state_mut();
            state.launcher_scope = Some(workbox.clone());

            let mut named = crate::state::Attachment::new();
            named.epoch = 11;
            named.session_name = Some("backend".into());
            named.session_attached = true;
            named.remote_host = Some("workbox".into());
            named.remote_target = Some(workbox.clone());
            named.session_client = Some(named_client);
            state.background.insert(11, named);

            // Auto-created and never engaged: the definition of disposable.
            let mut scratch = crate::state::Attachment::new();
            scratch.epoch = 12;
            scratch.session_name = Some(crate::state::remote_ephemeral_session_name());
            scratch.session_attached = true;
            scratch.auto_created = true;
            scratch.engaged = false;
            scratch.remote_host = Some("workbox".into());
            scratch.remote_target = Some(workbox.clone());
            scratch.session_client = Some(scratch_client);
            state.background.insert(12, scratch);

            // The surface `Ctrl+X` is pressed on: the workbox Sessions tab.
            state.session_picker =
                Some(SessionPickerState::new(Vec::new()).on_tab(Some(workbox.clone())));
            state.show_session_picker = true;
        }

        backend
            .update_level(Msg::SessionPickerDisconnectHost)
            .expect("disconnect the host from its own picker");

        let named_sent: Vec<ClientOutbound> = named_rx.try_iter().collect();
        assert!(
            named_sent
                .iter()
                .any(|message| matches!(message, ClientOutbound::Control(ClientMessage::Detach))),
            "a named remote session is detached, not ended: {named_sent:?}"
        );
        assert!(
            !named_sent
                .iter()
                .any(|message| matches!(message, ClientOutbound::Control(ClientMessage::Shutdown))),
            "disconnecting a host must never kill a named session"
        );

        let scratch_sent: Vec<ClientOutbound> = scratch_rx.try_iter().collect();
        assert!(
            scratch_sent
                .iter()
                .any(|message| matches!(message, ClientOutbound::Control(ClientMessage::Shutdown))),
            "an untouched scratch session on the host we are leaving is closed: {scratch_sent:?}"
        );

        let state = backend.state();
        assert!(
            state.background.is_empty(),
            "no attachment survives the host"
        );
        assert!(
            state.launcher_scope.is_none(),
            "the launcher stops being scoped to a host we just disconnected from"
        );
        assert!(
            state.replaceable_toasts.is_empty(),
            "the picker and host status already show the disconnect"
        );
    });
}

/// The launcher scope is the launcher's, not the overlay's. Dismissing the workbox Sessions tab is not a withdrawal of the request to work on `workbox`, so the client
/// that dismisses the picker is still standing on the machine it opened.
#[test]
fn backing_out_of_a_host_keeps_the_launcher_scoped_to_it() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionPickerState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
        {
            let state = backend.state_mut();
            // A launch seeds an initial pane; the launcher is what is left once nothing is in
            // front of it.
            *state.current_mut() = crate::state::Attachment::new();
            state.launcher_scope = Some(workbox.clone());
            state.session_picker =
                Some(SessionPickerState::new(Vec::new()).on_tab(Some(workbox.clone())));
            state.show_session_picker = true;
        }
        assert!(backend.state().is_launcher());

        backend
            .update_level(Msg::CloseSessionPicker)
            .expect("dismiss host sessions");

        let state = backend.state();
        assert!(
            state.remote_picker.is_none() && !state.show_session_picker,
            "Esc closes the shared Sessions picker"
        );
        assert_eq!(
            state.active_launcher_scope(),
            Some(&workbox),
            "the launcher still names the host that was opened"
        );
    });
}

#[test]
fn disconnecting_the_launcher_picker_host_moves_to_local_before_starting_a_shell() {
    use crate::{AppRoot, Msg};
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let mut backend = TestBackend::new(AppRoot::default());
        let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
        {
            let state = backend.state_mut();
            *state.current_mut() = crate::state::Attachment::new();
            state.launcher_scope = Some(workbox.clone());
            state.session_picker = Some(
                SessionPickerState::new(vec![
                    session_row("backend", Some("workbox")),
                    session_row("local-dev", None),
                ])
                .on_tab(Some(workbox)),
            );
            state.show_session_picker = true;
        }

        backend
            .update_level(Msg::SessionPickerDisconnectHost)
            .expect("disconnect the launcher picker's host");

        assert!(backend.state().launcher_scope.is_none());
        let picker = backend.state().session_picker.as_ref().unwrap();
        assert_eq!(picker.tab, crate::state::SessionPickerTab::Host(None));
        assert_eq!(picker.entries[picker.selected].name, "local-dev");

        backend
            .update_level(Msg::SessionPickerEphemeral)
            .expect("start a local scratch session");
        let pending = backend
            .state()
            .current()
            .pending_session_attach
            .as_ref()
            .expect("local attach is pending");
        assert_eq!(pending.request.name, crate::state::ephemeral_session_name());
        assert!(pending.request.remote.is_none());
        assert!(backend.state().current().remote_target.is_none());
    });
}

/// A scope is a tie to a host with no attachment behind it, and `Ctrl+X` is the key that cuts one.
/// Offering it only for held sessions would leave `REMOTE · workbox` — the one state the badge
/// exists to describe — as the one state with no way out of it.
#[test]
fn a_host_held_only_by_the_launcher_scope_can_still_be_disconnected() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionPickerState;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        crate::test_support::isolate_user_dirs();
        for query in ["", "missing"] {
            let mut backend = TestBackend::new(AppRoot::default());
            let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
            {
                let state = backend.state_mut();
                *state.current_mut() = crate::state::Attachment::new();
                state.launcher_scope = Some(workbox.clone());
                state.session_picker =
                    Some(SessionPickerState::new(Vec::new()).on_tab(Some(workbox.clone())));
                state.session_picker.as_mut().unwrap().input.set_text(query);
                state.config.animations.picker = crate::layout::anim::PickerAnimationStyle::Off;
                state.show_session_picker = true;
            }
            assert!(
                crate::ops::session::host_can_disconnect(backend.state(), &workbox),
                "the scope alone is enough to have something to disconnect"
            );

            backend.render();
            let rendered = backend.capture_frame().to_fixed_grid();
            assert!(
                rendered.contains("disconnect host"),
                "query {query:?}: {rendered}"
            );

            backend
                .update_level(Msg::SessionPickerDisconnectHost)
                .expect("disconnect a host we only point at");

            assert!(
                backend.state().launcher_scope.is_none(),
                "the launcher stops naming a host we just walked away from"
            );
            assert!(
                !crate::ops::session::host_can_disconnect(backend.state(), &workbox),
                "with the scope gone there is nothing left to disconnect"
            );
        }
    });
}

/// The scope is only a scope while the foreground is empty, so a host merely recorded behind a
/// live session is not something `Ctrl+X` offers to disconnect from.
#[test]
fn a_recorded_scope_behind_a_live_session_is_not_a_host_connection() {
    let workbox = crate::session::remote::RemoteTarget::Alias("workbox".into());
    let mut state = State::new(Config::default(), ThemePreset::Lipan.theme());
    *state.current_mut() = crate::state::Attachment::new();
    state.launcher_scope = Some(workbox.clone());
    assert!(crate::ops::session::host_can_disconnect(&state, &workbox));

    // A local session in front: the field is now a record of where the user was working.
    state.current_mut().session_name = Some("dev".into());
    assert!(!crate::ops::session::host_can_disconnect(&state, &workbox));
}

/// Forgetting a last-seen row drops that session from the host cache and off the picker. It does
/// not hide the name forever: writing the same observation back lists it again.
#[test]
fn forgetting_a_last_seen_row_drops_only_that_cached_observation() {
    use crate::AppRoot;
    use crate::session::CachedHostSession;
    use tui_lipan::TestBackend;

    crate::test_support::on_large_stack(|| {
        let _persist = crate::test_support::lock_persisted_state();
        let target = crate::session::remote::RemoteTarget::Alias("winvm".to_string());
        let remembered = crate::session::discovery::DiscoveredSession {
            name: "test".to_string(),
            origin: Default::default(),
            ephemeral: false,
            host: Some("winvm".to_string()),
            remote_target: Some(target.clone()),
            status: crate::session::discovery::DiscoveredSessionStatus::LastSeen { panes: 1 },
        };
        let sibling = CachedHostSession {
            name: "keep".to_string(),
            origin: Default::default(),
            ephemeral: false,
            panes: 2,
        };
        let mut backend = TestBackend::new(AppRoot::default());
        {
            let state = backend.state_mut();
            *state.current_mut() = crate::state::Attachment::new();
            state.show_session_picker = true;
            state.config.remote.hosts.insert(
                "winvm".to_string(),
                crate::config::RemoteHostConfig::default(),
            );
            state.remote.hosts.seed(&state.config.remote, &[], &[], &[]);
            crate::session::set_cached_host_sessions(
                &mut state.remote.session_cache,
                &target,
                vec![
                    CachedHostSession {
                        name: "test".to_string(),
                        origin: Default::default(),
                        ephemeral: false,
                        panes: 1,
                    },
                    sibling.clone(),
                ],
            );
            // A remembered row is only listed under a host tab, here the one opened explicitly.
            state.session_picker =
                Some(SessionPickerState::new(vec![remembered]).on_tab(Some(target.clone())));
        }

        backend
            .dispatch(crate::Msg::SessionPickerKillSelected)
            .expect("arm forget");
        assert_eq!(
            backend
                .state()
                .session_picker
                .as_ref()
                .and_then(|picker| picker.pending_kill),
            Some(0)
        );

        backend
            .dispatch(crate::Msg::SessionPickerKillSelected)
            .expect("confirm forget");

        let cache =
            crate::session::host_sessions_for(&backend.state().remote.session_cache, &target)
                .unwrap_or_default();
        assert!(
            cache.iter().all(|session| session.name != "test"),
            "the forgotten observation is gone: {cache:?}"
        );
        assert!(
            cache.iter().any(|session| session.name == "keep"),
            "a sibling on the same host is left alone: {cache:?}"
        );
        let picker = backend
            .state()
            .session_picker
            .as_ref()
            .expect("picker stays open");
        assert!(
            picker.entries.iter().all(|entry| entry.name != "test"),
            "the row leaves the picker: {:?}",
            picker.entries
        );

        let restored = vec![
            CachedHostSession {
                name: "test".to_string(),
                origin: Default::default(),
                ephemeral: false,
                panes: 1,
            },
            sibling,
        ];
        crate::session::record_host_sessions(&target, restored.clone());
        crate::session::set_cached_host_sessions(
            &mut backend.state_mut().remote.session_cache,
            &target,
            restored,
        );
        let epoch = backend.state().session_picker_epoch;
        backend
            .update_level(crate::Msg::SessionsDiscovered {
                epoch,
                rows: Vec::new(),
                host_status: Vec::new(),
            })
            .expect("replay a later probe");
        let picker = backend.state().session_picker.as_ref().expect("picker");
        assert!(
            picker.entries.iter().any(|entry| {
                entry.name == "test"
                    && matches!(
                        entry.status,
                        crate::session::discovery::DiscoveredSessionStatus::LastSeen { .. }
                    )
            }),
            "a rediscovered session is listed again: {:?}",
            picker.entries
        );
    });
}

#[test]
fn remote_kill_result_updates_live_rows_without_waiting_for_host_monitor() {
    crate::test_support::on_large_stack(|| {
        let _persist = crate::test_support::lock_persisted_state();
        for succeeds in [true, false] {
            let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
            let killed = session_row("dev", Some("workbox"));
            let sibling = session_row("keep", Some("workbox"));
            let elsewhere = session_row("dev", Some("otherbox"));
            let target = killed.remote_target.clone().unwrap();
            {
                let state = backend.state_mut();
                *state.current_mut() = crate::state::Attachment::new();
                state.command_link = None;
                for host in ["workbox", "otherbox"] {
                    state
                        .config
                        .remote
                        .hosts
                        .insert(host.into(), crate::config::RemoteHostConfig::default());
                }
                state
                    .remote
                    .monitors
                    .push(crate::session::remote::monitor::Monitor::dormant(
                        target.clone(),
                        7,
                    ));
                state
                    .remote
                    .monitors
                    .push(crate::session::remote::monitor::Monitor::dormant(
                        elsewhere.remote_target.clone().unwrap(),
                        8,
                    ));
                state.show_session_picker = true;
                state.remote.live_sessions =
                    vec![killed.clone(), sibling.clone(), elsewhere.clone()];
                state
                    .remote
                    .killing_sessions
                    .push((killed.name.clone(), target.clone()));
                state.remote.hosts.seed(
                    &state.config.remote,
                    std::slice::from_ref(&target),
                    &[],
                    &[],
                );
                state.remote.hosts.get_mut(&target).unwrap().probe =
                    crate::state::HostProbe::Reached;
                state
                    .remote
                    .hosts
                    .get_mut(elsewhere.remote_target.as_ref().unwrap())
                    .unwrap()
                    .probe = crate::state::HostProbe::Reached;
                state.session_picker = Some(
                    SessionPickerState::new(state.remote.live_sessions.clone())
                        .on_tab(Some(target.clone())),
                );
                let cached = crate::ops::session::discovery::cached_sessions_for_target(
                    &state.remote.live_sessions,
                    &target,
                );
                crate::session::record_host_sessions(&target, cached.clone());
                crate::session::set_cached_host_sessions(
                    &mut state.remote.session_cache,
                    &target,
                    cached,
                );
            }
            // A pending shutdown cannot be armed again or activated while awaiting its result.
            backend
                .dispatch(crate::Msg::SessionPickerKillSelected)
                .unwrap();
            backend
                .dispatch(crate::Msg::SessionPickerActivate(0))
                .unwrap();
            assert!(
                backend
                    .state()
                    .session_picker
                    .as_ref()
                    .unwrap()
                    .pending_kill
                    .is_none()
            );
            assert!(backend.state().current().pending_session_attach.is_none());
            backend.state_mut().command_link = None;
            backend
                .dispatch(crate::Msg::RemoteSessionKilled {
                    entry: killed.clone(),
                    result: if succeeds {
                        Ok(())
                    } else {
                        Err("shutdown refused".into())
                    },
                })
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .remote
                    .monitors
                    .iter()
                    .any(|monitor| monitor.target == target),
                !succeeds
            );
            assert!(
                backend
                    .state()
                    .remote
                    .monitors
                    .iter()
                    .any(|monitor| Some(&monitor.target) == elsewhere.remote_target.as_ref())
            );
            if succeeds {
                // Model sync replacing the canceled monitor without starting an SSH worker.
                backend.state_mut().remote.monitors.push(
                    crate::session::remote::monitor::Monitor::dormant(target.clone(), 9),
                );
            }
            // This snapshot began before shutdown completed and arrived after its result.
            backend
                .dispatch(crate::Msg::HostMetadata {
                    target: target.clone(),
                    generation: 7,
                    rows: Ok(vec![killed.clone(), sibling.clone()]),
                    agents: Vec::new(),
                    projects: Vec::new(),
                })
                .unwrap();
            let state = backend.state();
            assert!(state.remote.killing_sessions.is_empty());
            assert_eq!(state.remote.live_sessions.contains(&killed), !succeeds);
            assert!(state.remote.live_sessions.contains(&sibling));
            assert!(state.remote.live_sessions.contains(&elsewhere));
            assert_eq!(
                    state
                        .sidebar
                        .sessions
                        .iter()
                        .any(|row| row.name == killed.name
                            && row.remote_target == killed.remote_target),
                    !succeeds
                );
            let persisted = crate::session::read_host_session_cache();
            assert_eq!(
                crate::session::host_sessions_for(&persisted, &target)
                    .unwrap()
                    .iter()
                    .any(|row| row.name == killed.name),
                !succeeds
            );
            let picker = state.session_picker.as_ref().unwrap();
            assert_eq!(picker.entries.contains(&killed), !succeeds);
            let cached =
                crate::session::host_sessions_for(&state.remote.session_cache, &target).unwrap();
            assert_eq!(
                cached.iter().any(|session| session.name == "dev"),
                !succeeds
            );
        }
    });
}

#[test]
fn killing_current_remote_session_removes_its_live_snapshot_immediately() {
    crate::test_support::on_large_stack(|| {
        let _persist = crate::test_support::lock_persisted_state();
        let mut backend = tui_lipan::TestBackend::new(crate::AppRoot::default());
        let killed = session_row("dev", Some("workbox"));
        let sibling = session_row("keep", Some("workbox"));
        {
            let state = backend.state_mut();
            *state.current_mut() = crate::state::Attachment::new();
            state.current_mut().session_name = Some(killed.name.clone());
            state.current_mut().remote_target = killed.remote_target.clone();
            state.current_mut().remote_host = killed.host.clone();
            state.current_mut().session_attached = true;
            state.command_link = None;
            state
                .remote
                .monitors
                .push(crate::session::remote::monitor::Monitor::dormant(
                    killed.remote_target.clone().unwrap(),
                    7,
                ));
            state.remote.live_sessions = vec![killed.clone(), sibling.clone()];
            state.show_session_picker = true;
            state.session_picker =
                Some(SessionPickerState::new(state.remote.live_sessions.clone()));
        }
        backend
            .dispatch(crate::Msg::SessionPickerKillSelected)
            .unwrap();
        backend
            .dispatch(crate::Msg::SessionPickerKillSelected)
            .unwrap();
        backend
            .dispatch(crate::Msg::HostMetadata {
                target: killed.remote_target.clone().unwrap(),
                generation: 7,
                rows: Ok(vec![killed.clone(), sibling.clone()]),
                agents: Vec::new(),
                projects: Vec::new(),
            })
            .unwrap();
        assert!(!backend.state().remote.live_sessions.contains(&killed));
        assert!(backend.state().remote.live_sessions.contains(&sibling));
        assert!(
            !backend
                .state()
                .session_picker
                .as_ref()
                .unwrap()
                .entries
                .contains(&killed)
        );
    });
}
