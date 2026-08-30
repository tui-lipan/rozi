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
        ephemeral: false,
        host: host.map(str::to_string),
        remote_target: host
            .map(|host| crate::session::remote::RemoteTarget::Alias(host.to_string())),
        status: crate::session::discovery::DiscoveredSessionStatus::Running {
            panes: 1,
            has_layout: true,
            clients: 1,
            created_from_profile: None,
        },
    }
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

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            let mut picker = SessionPickerState::new(vec![
                session_row("alpha", None),
                session_row("zulu", None),
            ]);
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
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
            ephemeral: false,
            panes: 4,
        }],
    );
    let mut rows = vec![session_row("local", None)];

    let mut hosts = crate::state::HostRegistry::default();
    hosts.seed(&config, &[], &[]);
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
    assert!(matches!(
        remote.status,
        crate::session::discovery::DiscoveredSessionStatus::Running { panes: 4, .. }
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
            ephemeral: false,
            panes: 2,
        }],
    );
    let mut rows = vec![session_row("live", Some("winvm"))];

    let mut hosts = crate::state::HostRegistry::default();
    hosts.seed(&config, &[], &[]);
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

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
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
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = shared_controller_backend();
            backend.state_mut().collaboration = Some(crate::state::CollaborationState::new());

            backend.render();
            let rendered = backend.capture_frame().to_fixed_grid();
            // Title and self-context share the top border, so neither costs a content row.
            assert!(
                rendered.contains("Manage collaborators"),
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
            assert!(rendered.contains("decline Ctrl+d"), "{rendered}");
            assert!(rendered.contains("kick Ctrl+k"), "{rendered}");
        })
        .expect("spawn collaborators view test")
        .join()
        .expect("collaborators view test completes");
}

/// Typing filters the roster instead of triggering actions: the letters that used to navigate
/// or act (`j`, `k`, `g`, `d`, `x`) must reach the query input now that it owns focus.
#[test]
fn plain_letters_reach_the_filter_instead_of_acting() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = shared_controller_backend();
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
        })
        .expect("spawn filter-typing test")
        .join()
        .expect("filter-typing test completes");
}

/// An empty list means two different things, and the message must not claim the wrong one: a
/// query that matched nobody is not the same as a session nobody else is on.
#[test]
fn an_empty_list_says_whether_it_is_the_filter_or_the_roster() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = shared_controller_backend();
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
        })
        .expect("spawn empty-text test")
        .join()
        .expect("empty-text test completes");
}

/// A query that hides every client must leave nothing to act on: the footer stops advertising
/// keys, and `ctrl+k` cannot reach a row scrolled out of sight by the filter.
#[test]
fn a_filter_that_hides_everyone_disarms_the_dialog() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
        })
        .expect("spawn hidden-filter test")
        .join()
        .expect("hidden-filter test completes");
}

/// Removing a client is destructive to somebody else's attachment, so the first press only arms
/// the row and nothing goes on the wire until the second one.
#[test]
fn kicking_a_collaborator_takes_two_presses() {
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
        })
        .expect("spawn kick test")
        .join()
        .expect("kick test completes");
}

/// The arming runs on the shared confirmation clock, so a kick left half-pressed lapses like
/// every other destructive gesture rather than waiting indefinitely for a second key.
#[test]
fn an_unconfirmed_kick_lapses_on_the_shared_confirm_window() {
    use crate::session::client::SessionClient;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
        })
        .expect("spawn kick-expiry test")
        .join()
        .expect("kick-expiry test completes");
}

#[test]
fn occupied_session_prompt_keeps_context_in_the_title() {
    use crate::AppRoot;
    use crate::state::FollowPromptState;
    use tui_lipan::TestBackend;
    use tui_lipan::prelude::Rect;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
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
        })
        .expect("spawn occupied-session prompt test")
        .join()
        .expect("occupied-session prompt test completes");
}

#[test]
fn cancelling_occupied_attach_does_not_retain_it_as_offline() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use crate::state::{Attachment, ConnectionState, FollowPromptState};
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                target_rx.try_iter().any(|message| matches!(
                    message,
                    ClientOutbound::Control(ClientMessage::Detach)
                ))
            );
        })
        .expect("spawn cancel attach test")
        .join()
        .expect("cancel attach test completes");
}

#[test]
fn killing_the_last_attached_session_stays_sessionless_without_auto_attach() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::bootstrap::has_session_candidates;
    use crate::session::client::SessionClient;
    use crate::state::ConnectionState;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                state.host_session_cache.clear();
                state.show_session_picker = false;
                state.session_picker = None;
            }

            backend
                .dispatch(Msg::RunAction(Action::KillSession))
                .expect("kill last session");

            let state = backend.state();
            assert!(state.is_launcher());
            assert!(state.current().pending_session_attach.is_none());
            // Other sessions on the host machine still count as choices; only a truly empty
            // discovery set keeps the picker closed.
            assert_eq!(state.show_session_picker, has_session_candidates());
        })
        .expect("spawn last-session kill test")
        .join()
        .expect("last-session kill test completes");
}

#[test]
fn starting_a_shell_from_the_picker_attaches_the_ephemeral_with_the_launch_seed() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionPickerState;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("dev", None)]));
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
            assert_eq!(pending.name, crate::state::ephemeral_session_name());
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
        })
        .expect("spawn picker start-shell test")
        .join()
        .expect("picker start-shell test completes");
}

#[test]
fn creating_a_session_with_an_existing_name_keeps_the_prompt_and_shows_an_inline_error() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::session::client::SessionClient;
    use crate::state::{ConnectionState, SessionRenameState};
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                    selected: 0,
                });
                state.rename_session =
                    Some(SessionRenameState::new("dev", NamingMode::CreateSession));
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
        })
        .expect("spawn colliding-create test")
        .join()
        .expect("colliding-create test completes");
}

#[test]
fn create_session_starts_fresh_instead_of_carrying_current_panes() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::state::SessionRenameState;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                state.sidebar_visible = true;
                state.sidebar.panels[0].active_tab =
                    Some(crate::config::SidebarTabId::new("sessions"));
                state.workbar.command_epoch = 3;
                state
                    .workbar
                    .command_in_flight
                    .insert("date".to_string(), 3);
                // Simulate a profile-seeded session: the current pane carries a command.
                state.current_mut().workspaces[0].panes[0].identity.launch =
                    Some(crate::pane_launch::PaneLaunch::shell("nvim"));
                state.rename_session =
                    Some(SessionRenameState::new(&name, NamingMode::CreateSession));
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
            assert_eq!(pending.name, name);
            assert_eq!(pending.intent, crate::state::AttachIntent::Plain);
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
            // untouched: command/config epochs don't churn (command tabs keep polling, no
            // flicker), the sidebar stays open on its tab, and workbar scheduling stays live.
            // This is the whole point of installing an attachment instead of rebuilding State.
            assert_eq!(state.sidebar.command_epoch, 7);
            assert_eq!(state.sidebar.config_epoch, 11);
            assert!(state.sidebar_visible);
            assert_eq!(
                state.sidebar.active_tab(),
                Some(&crate::config::SidebarTabId::new("sessions"))
            );
            assert_eq!(state.workbar.command_epoch, 3);
            assert_eq!(state.workbar.command_in_flight.get("date"), Some(&3));
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn controller_grant_control_key_grants_to_the_earliest_requester() {
    use crate::AppRoot;
    use crate::Msg;
    use crate::input::Action;
    use crate::session::client::{ClientOutbound, SessionClient};
    use crate::session::protocol::ClientMessage;
    use tui_lipan::TestBackend;

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
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
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
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
                state.rename_session =
                    Some(SessionRenameState::new(&name, NamingMode::CreateSession));
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
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

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("dev", None)]));
                state.show_session_picker = true;
            }

            backend
                .dispatch(Msg::SessionPickerActivate(0))
                .expect("attach to parked session");

            assert_eq!(
                backend.state().current().session_name.as_deref(),
                Some("dev")
            );
            assert!(!backend.state().show_profile_picker);
            assert!(backend.state().profile_picker.is_none());
            assert!(!backend.state().show_session_picker);
            assert!(backend.state().session_picker.is_none());
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
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

    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("dev", None)]));
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
                    .is_some_and(|pending| pending.name == "dev")
            );
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn activating_restorable_session_autostarts_its_server() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
            assert_eq!(pending.name, "saved");
            assert!(pending.autostart);
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn restart_is_inert_for_a_restorable_session() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
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
                    .is_none_or(|pending| pending.name != "saved"),
                "restart must not restore the snapshot"
            );
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn activating_the_current_session_is_inert() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use crate::AppRoot;
            use crate::Msg;
            use tui_lipan::TestBackend;

            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().session_attached = true;
                state.current_mut().pending_session_attach = None;
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("dev", None)]));
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn disconnecting_the_current_session_is_inert() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use crate::AppRoot;
            use crate::Msg;
            use tui_lipan::TestBackend;

            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.current_mut().session_name = Some("dev".into());
                state.current_mut().session_attached = true;
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("dev", None)]));
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}

#[test]
fn disconnecting_a_session_we_do_not_hold_is_inert() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            use crate::AppRoot;
            use crate::Msg;
            use tui_lipan::TestBackend;

            let mut backend = TestBackend::new(AppRoot::default());
            {
                let state = backend.state_mut();
                state.current_mut().session_name = Some("here".into());
                state.current_mut().session_attached = true;
                state.session_picker =
                    Some(SessionPickerState::new(vec![session_row("there", None)]));
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
        })
        .expect("spawn test thread")
        .join()
        .expect("test thread panicked");
}
