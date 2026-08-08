//! Sessions sidebar hierarchy, color, and host action presentation.

use hyprmux::HyprmuxApp;
use hyprmux::config::{RemoteHostConfig, SidebarTab, SidebarTabId};
use hyprmux::session::CachedHostSession;
use hyprmux::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use hyprmux::session::remote::RemoteTarget;
use hyprmux::state::HostProbe;
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
    backend: &mut TestBackend<HyprmuxApp>,
    what: &str,
    mut condition: impl FnMut(&mut TestBackend<HyprmuxApp>) -> bool,
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
            let mut backend = TestBackend::new(HyprmuxApp::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                state.sidebar_visible = true;
                state.config.sidebar.tabs = vec![SidebarTab::Sessions];
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

            let linvm = row("LINVM");
            assert!(lines[linvm].starts_with(" LINVM"));
            assert!(lines[linvm + 1].starts_with(" Click to disconnect"));
            assert!(lines[linvm + 2].starts_with("  dev"));
            assert!(lines[linvm + 3].starts_with("  4 panes"));

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
            assert_eq!(frame.cell(1, (winvm + 1) as u16).fg, muted);
            assert_eq!(frame.cell(2, (winvm + 3) as u16).fg, muted);

            let move_to = |backend: &mut TestBackend<HyprmuxApp>, y: u16| {
                backend
                    .send_mouse(MouseEvent {
                        x: 4,
                        y,
                        kind: MouseKind::Moved,
                        mods: KeyMods::NONE,
                    })
                    .expect("move over host row");
                settle_until(backend, "the pointer to land on a host row", |backend| {
                    let panel = &backend.state().sidebar.panels[0];
                    panel.hovered_row.is_some() && !panel.suppress_row_hover
                });
            };

            // Both rows of a host group resolve to one hovered row, so pointing at either the
            // header or the line under it selects the same row and leaves hover unsuppressed.
            //
            // The painted lift is deliberately not asserted here. Its background transform is
            // gathered during mouse dispatch (`app::input::mouse::gather`) and consumed at render,
            // so whether a given captured frame carries it depends on how dispatch and render
            // interleave - stable on a developer machine, not on a loaded CI runner, where this
            // read the unlifted colour while the sidebar's own state already said the row was
            // hovered. The state below is what every hover-dependent behaviour in `view::sidebar`
            // actually branches on. Asserting the colour needs a hook that resolves hover at
            // capture time rather than a second guess at the theme arithmetic.
            move_to(&mut backend, linvm as u16);
            let header_row = backend.state().sidebar.panels[0].hovered_row;
            assert!(header_row.is_some(), "host header should hover a row");

            move_to(&mut backend, linvm as u16 + 1);
            assert_eq!(
                backend.state().sidebar.panels[0].hovered_row,
                header_row,
                "the line under a host header belongs to the same hovered row"
            );
            assert!(!backend.state().sidebar.panels[0].suppress_row_hover);
        })
        .expect("spawn Sessions sidebar smoke thread")
        .join()
        .expect("Sessions sidebar smoke completes");
}
