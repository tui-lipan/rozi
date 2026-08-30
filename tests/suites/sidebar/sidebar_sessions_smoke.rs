//! Sessions sidebar hierarchy, color, and host action presentation.

use rozi::AppRoot;
use rozi::config::{RemoteHostConfig, SidebarTab, SidebarTabId};
use rozi::session::CachedHostSession;
use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::remote::RemoteTarget;
use rozi::state::HostProbe;
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseEvent, MouseKind};
use tui_lipan::prelude::{KeyMods, Rect};

fn session(name: &str, panes: usize, clients: u32, host: Option<&str>) -> DiscoveredSession {
    DiscoveredSession {
        name: name.into(),
        status: DiscoveredSessionStatus::Running {
            panes,
            clients,
            has_layout: true,
            created_from_profile: None,
        },
        ephemeral: false,
        host: host.map(str::to_string),
        remote_target: host.map(|host| RemoteTarget::Alias(host.into())),
    }
}

/// Render and pump until `condition` holds, instead of a fixed number of rounds.
///
/// `pump` drains the queued messages but does not wait for background work, so a fixed count is a
/// race rather than a settle: on a loaded runner the repaint that applies hover can land after the
/// last round, and the assertion then reads the pre-hover cell. Waiting on the observable condition
/// removes the timing assumption without weakening what is asserted - a hover that never arrives
/// still fails, just with a message saying so.
fn settle_until(
    backend: &mut TestBackend<AppRoot>,
    what: &str,
    mut condition: impl FnMut(&mut TestBackend<AppRoot>) -> bool,
) {
    // Generous for the same reason as the integration harness's IO_TIMEOUT: the wait ends as soon
    // as the condition holds, so this only bounds how long a real failure takes to surface.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        backend.render();
        let _ = backend.pump();
        backend.render();
        if condition(backend) {
            return;
        }
        if std::time::Instant::now() >= deadline {
            // Row hover is gated on `suppress_row_hover`, which keyboard cursor movement sets and a
            // real pointer move clears, so report that gate rather than only the failed comparison.
            let panels: Vec<_> = backend
                .state()
                .sidebar
                .panels
                .iter()
                .map(|panel| {
                    format!(
                        "{{hovered_row: {:?}, suppress_row_hover: {}, cursor: {}}}",
                        panel.hovered_row, panel.suppress_row_hover, panel.cursor
                    )
                })
                .collect();
            panic!(
                "timed out waiting for {what}; sidebar panels: {}",
                panels.join(", ")
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[test]
fn sessions_sidebar_renders_group_and_child_hierarchy() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                // Revealing the sidebar after the first frame is a real toggle, so it runs the
                // real slide; these assertions are about the settled column, not a frame
                // part-way through it.
                state.config.animations.sidebar = false;
                // The hierarchy needs every row on screen, so collapse the default split and give
                // the Sessions tab the whole sidebar.
                state.config.sidebar.tabs = vec![SidebarTab::Sessions];
                state.config.sidebar.split = false;
                state.sidebar.apply_configured_panels(&state.config.sidebar);
                state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
                state
                    .config
                    .remote
                    .hosts
                    .insert("linvm".into(), RemoteHostConfig::default());
                state
                    .config
                    .remote
                    .hosts
                    .insert("winvm".into(), RemoteHostConfig::default());
                state.hosts.seed(&state.config.remote, &[], &[]);
                state
                    .hosts
                    .get_mut(&RemoteTarget::Alias("linvm".into()))
                    .expect("linvm")
                    .probe = HostProbe::Reached;
                state.current_mut().session_name = Some("dev".into());
                state.sidebar.sessions = vec![
                    session("dev", 2, 2, None),
                    session("test", 3, 0, None),
                    session("dev", 4, 0, Some("linvm")),
                ];
                state.host_session_cache.insert(
                    "winvm".into(),
                    vec![CachedHostSession {
                        name: "dev".into(),
                        ephemeral: false,
                        panes: 2,
                    }],
                );
            }

            backend.render();
            let frame = backend.capture_frame();
            let lines: Vec<String> = frame
                .to_fixed_grid_lines()
                .into_iter()
                .map(|line| line.chars().take(32).collect())
                .collect();

            let row = |needle: &str| {
                lines
                    .iter()
                    .position(|line| line.contains(needle))
                    .unwrap_or_else(|| panic!("{needle:?} should be visible: {lines:#?}"))
            };

            assert!(lines[row("LOCAL")].starts_with(" LOCAL"));
            assert!(lines[row("▍ dev")].starts_with("▍ dev"));
            assert!(lines[row("▍ dev")].contains("󰍺 2"));
            assert!(lines[row("2 panes")].starts_with("▍ 2 panes"));
            assert!(!lines[row("2 panes")].contains("shared"));
            assert!(lines[row("test")].starts_with("  test"));
            assert!(lines[row("3 panes")].starts_with("  3 panes"));

            let new_session_rows: Vec<_> = lines
                .iter()
                .filter(|line| line.contains("+ New session"))
                .collect();
            assert_eq!(new_session_rows.len(), 2);
            assert!(
                new_session_rows
                    .iter()
                    .all(|line| line.starts_with("  + New session"))
            );

            // A connected host is one line: the badge says it is connected, and disconnecting is
            // its hover ✕, so its sessions follow immediately.
            let linvm = row("LINVM");
            assert!(lines[linvm].starts_with(" LINVM"));
            assert!(
                !lines[linvm + 1].contains("Click to disconnect"),
                "no second line under a connected host: {:?}",
                lines[linvm + 1]
            );
            assert!(lines[linvm + 1].starts_with("  dev"));
            assert!(lines[linvm + 2].starts_with("  4 panes"));

            // An offline host with nothing to explain spends its free second line on the standing
            // invitation, rather than hiding it under the pointer.
            let winvm = row("WINVM");
            assert!(lines[winvm].starts_with(" WINVM"));
            assert!(lines[winvm + 1].starts_with(" Click to connect"));
            assert!(lines[winvm + 2].starts_with("  dev"));
            assert!(lines[winvm + 3].starts_with("  2 panes · last seen"));
            assert!(lines[row("+ Connect a host")].starts_with(" + Connect a host"));

            let accent = backend
                .state()
                .theme
                .accent
                .fg
                .expect("accent foreground")
                .color();
            let muted = backend
                .state()
                .theme
                .muted
                .fg
                .expect("muted foreground")
                .color();
            assert_eq!(frame.cell(1, linvm as u16).fg, accent);
            // The cached session under an offline host, and its detail: both muted, so a memory of
            // a session never reads like a live one.
            assert_eq!(frame.cell(1, (winvm + 1) as u16).fg, muted);
            assert_eq!(frame.cell(2, (winvm + 2) as u16).fg, muted);
            assert_eq!(frame.cell(2, (winvm + 3) as u16).fg, muted);
        })
        .expect("spawn Sessions sidebar smoke thread")
        .join()
        .expect("Sessions sidebar smoke completes");
}

/// The Sessions tab with one configured host in `probe` state and nothing else, so the host row
/// and whatever rides under it are the only things on screen.
fn host_backend(probe: HostProbe) -> TestBackend<AppRoot> {
    // Without this the sweep discovers whatever sessions other test binaries left in the real
    // runtime directory, which shifts every row under the pointer mid-test.
    rozi::test_support::isolate_user_dirs();
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(Rect {
        x: 0,
        y: 0,
        w: 100,
        h: 30,
    });
    let state = backend.state_mut();
    state.sidebar_visible = true;
    state.config.animations.sidebar = false;
    state.config.sidebar.tabs = vec![SidebarTab::Sessions];
    state.config.sidebar.split = false;
    state.sidebar.apply_configured_panels(&state.config.sidebar);
    state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("sessions"));
    state
        .config
        .remote
        .hosts
        .insert("workbox".into(), RemoteHostConfig::default());
    state.hosts.seed(&state.config.remote, &[], &[]);
    state
        .hosts
        .get_mut(&RemoteTarget::Alias("workbox".into()))
        .expect("workbox")
        .probe = probe;
    backend
}

fn sidebar_lines(backend: &mut TestBackend<AppRoot>) -> Vec<String> {
    backend.render();
    backend
        .capture_frame()
        .to_fixed_grid_lines()
        .into_iter()
        .map(|line| line.chars().take(32).collect())
        .collect()
}

/// Put the pointer on line `y` and wait for the sidebar to register the hover.
fn hover_row(backend: &mut TestBackend<AppRoot>, y: u16) -> Option<usize> {
    backend
        .send_mouse(MouseEvent {
            x: 4,
            y,
            kind: MouseKind::Moved,
            mods: KeyMods::NONE,
        })
        .expect("move over the row");
    settle_until(backend, "the pointer to land on a row", |b| {
        let panel = &b.state().sidebar.panels[0];
        panel.hovered_row.is_some() && !panel.suppress_row_hover
    });
    backend.state().sidebar.panels[0].hovered_row
}

fn host_row_index(lines: &[String]) -> usize {
    lines
        .iter()
        .position(|line| line.contains("WORKBOX"))
        .unwrap_or_else(|| panic!("the host row is listed: {lines:#?}"))
}

/// A host that failed spends its second line on the reason, so its connect affordance hides in the
/// badge's slot instead of growing the row to three lines.
#[test]
fn a_failed_host_offers_the_connect_affordance_in_place_of_its_status() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = host_backend(HostProbe::Failed(
                "bash: line 1: rozi: command not found".into(),
            ));
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);
            assert!(lines[host].contains("Unreachable"), "{:?}", lines[host]);
            assert!(
                !lines[host].contains("Connect"),
                "nothing about connecting until the pointer arrives: {:?}",
                lines[host]
            );

            hover_row(&mut backend, host as u16);

            // Re-resolve: settling runs a session sweep that can add rows above the host.
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);
            assert!(
                lines[host].contains("Connect"),
                "the affordance arrives on hover: {:?}",
                lines[host]
            );
            assert!(
                !lines[host].contains("Unreachable"),
                "in the badge's place, not beside it: {:?}",
                lines[host]
            );
            assert!(
                lines[host + 1].contains("No rozi on host"),
                "and the reason keeps its line: {:?}",
                lines[host + 1]
            );
        })
        .expect("spawn thread")
        .join()
        .expect("failed host affordance completes");
}

/// An offline host with nothing to explain has its second line free, so the invitation stands
/// there rather than waiting for the pointer.
#[test]
fn an_offline_host_keeps_a_standing_invitation_to_connect() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = host_backend(HostProbe::Idle);
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);
            assert!(lines[host].contains("Disconnected"), "{:?}", lines[host]);
            assert!(
                lines[host + 1].contains("Click to connect"),
                "the invitation stands without hovering: {:?}",
                lines[host + 1]
            );
        })
        .expect("spawn thread")
        .join()
        .expect("offline host invitation completes");
}

/// Why the last attempt failed is the one thing the badge cannot say, so it earns the second line —
/// on the host's own row, not a third row of its own.
#[test]
fn a_failed_probe_explains_itself_on_the_host_row() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = host_backend(HostProbe::Failed(
                "bash: line 1: rozi: command not found".into(),
            ));
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);

            assert!(lines[host].contains("Unreachable"), "{:?}", lines[host]);
            assert!(
                lines[host + 1].contains("No rozi on host"),
                "the reason rides directly under the host: {:?}",
                &lines[host..host + 2]
            );

            // Both lines are one row, so pointing at the reason aims at the host it belongs to
            // rather than at nothing.
            let host = host_row_index(&sidebar_lines(&mut backend)) as u16;
            let title_row = hover_row(&mut backend, host);
            let host = host_row_index(&sidebar_lines(&mut backend)) as u16;
            let reason_row = hover_row(&mut backend, host + 1);
            assert_eq!(
                title_row, reason_row,
                "the reason belongs to the host's row"
            );
        })
        .expect("spawn thread")
        .join()
        .expect("probe failure line completes");
}

/// Hover has to be tracked for every row, not only the ones a click activates.
///
/// Connecting takes the host row out of the selectable set, so a row that stopped being selectable
/// while the pointer sat on it never fired a leave — and the stale index outlived the pointer. The
/// failed host then came back wearing its hover affordance instead of its status.
#[test]
fn a_row_that_stops_being_selectable_still_releases_the_pointer() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = host_backend(HostProbe::Idle);
            let target = RemoteTarget::Alias("workbox".into());
            let host = host_row_index(&sidebar_lines(&mut backend)) as u16;
            hover_row(&mut backend, host);

            // Connecting: the row goes inert, which used to take its hover region with it.
            backend
                .state_mut()
                .hosts
                .get_mut(&target)
                .expect("workbox")
                .probe = HostProbe::InFlight;
            // Render it: the pointer acts on the tree that is actually on screen, so the row has
            // to have already lost its region before the pointer leaves — which is the whole
            // sequence being reproduced.
            let _ = sidebar_lines(&mut backend);

            // The pointer leaves the sidebar entirely.
            backend
                .send_mouse(MouseEvent {
                    x: 90,
                    y: 20,
                    kind: MouseKind::Moved,
                    mods: KeyMods::NONE,
                })
                .expect("move off the sidebar");
            settle_until(&mut backend, "the pointer to leave every row", |b| {
                b.state().sidebar.panels[0].hovered_row.is_none()
            });

            // The probe fails; the row is selectable again and must read as unreachable.
            backend
                .state_mut()
                .hosts
                .get_mut(&target)
                .expect("workbox")
                .probe = HostProbe::Failed("bash: rozi: command not found".into());
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);
            assert!(
                lines[host].contains("Unreachable"),
                "the status is back, not the hover affordance: {:?}",
                lines[host]
            );
            assert!(!lines[host].contains("Connect"), "{:?}", lines[host]);
        })
        .expect("spawn thread")
        .join()
        .expect("hover release completes");
}

/// A connected host is inert — clicking it does nothing — but it is still closable, and the ✕ is
/// the only way to disconnect it. Hover therefore has to reach a row that has no activation.
#[test]
fn a_connected_host_reveals_its_disconnect_affordance_on_hover() {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = host_backend(HostProbe::Reached);
            let host = host_row_index(&sidebar_lines(&mut backend)) as u16;
            assert!(
                !sidebar_lines(&mut backend)[host as usize].contains('✕'),
                "quiet at rest"
            );

            hover_row(&mut backend, host);
            let lines = sidebar_lines(&mut backend);
            let host = host_row_index(&lines);
            assert!(
                lines[host].contains('✕'),
                "an inert row still takes the pointer: {:?}",
                lines[host]
            );
        })
        .expect("spawn thread")
        .join()
        .expect("connected host affordance completes");
}
