//! Visual reference sketches: headless renders of chrome whose correctness is a colour or layout
//! judgement rather than an assertion, written as PNGs for side-by-side review.
//!
//! These used to be `#[test]` functions with no assertions. They are tools, not regressions, so they
//! live here and run only when asked:
//!
//! ```bash
//! cargo run --features ui-snapshot --example ui_sketches            # list scenarios
//! cargo run --features ui-snapshot --example ui_sketches -- all
//! cargo run --features ui-snapshot --example ui_sketches -- worktree-picker
//! ```
//!
//! Output goes to `target/ui-sketches/`.

use std::path::{Path, PathBuf};

use rozi::config::{SidebarTab, SidebarTabId};
use rozi::git::worktrees::{WorktreeInfo, WorktreeLock};
use rozi::session::discovery::{DiscoveredSession, DiscoveredSessionStatus};
use rozi::session::origin::{SessionOrigin, WorktreeOrigin};
use rozi::session::protocol::WorktreeSession;
use rozi::state::{AlertMode, Pane, WorktreePickerState};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::core::event::{MouseEvent, MouseKind};
use tui_lipan::prelude::{FloatRect, KeyMods, Rect};

const SCENARIOS: [(&str, &str, fn()); 4] = [
    (
        "floating-title-caps",
        "floating titlebar caps over dim and decorated terminal text",
        floating_title_caps,
    ),
    (
        "workbar-alerts",
        "workspace-tab alert markers at both ends of the breathe, and hovered",
        workbar_alerts,
    ),
    (
        "sidebar-worktrees",
        "the Worktrees sidebar tab with a hovered and an armed row",
        sidebar_worktrees,
    ),
    (
        "worktree-picker",
        "the worktree picker at three sizes, and its new-checkout form",
        worktree_picker,
    ),
];

fn main() {
    let requested: Vec<String> = std::env::args().skip(1).collect();
    if requested.is_empty() {
        println!("usage: ui_sketches <scenario>... | all\n");
        for (name, about, _) in SCENARIOS {
            println!("  {name:<18} {about}");
        }
        return;
    }
    let all = requested.iter().any(|name| name == "all");
    for name in &requested {
        if name != "all" && !SCENARIOS.iter().any(|(known, _, _)| known == name) {
            eprintln!("unknown scenario {name:?}; run without arguments to list them");
            std::process::exit(2);
        }
    }
    rozi::test_support::isolate_user_dirs();
    for (name, _, run) in SCENARIOS {
        if all || requested.iter().any(|requested| requested == name) {
            // Rendering the full app tree needs more stack than a default thread has.
            std::thread::Builder::new()
                .stack_size(16 * 1024 * 1024)
                .spawn(run)
                .expect("spawn sketch thread")
                .join()
                .expect("sketch completes");
        }
    }
}

fn write_png(backend: &mut TestBackend<AppRoot>, name: &str) {
    let png = backend
        .capture_ui_snapshot()
        .to_png_default()
        .expect("encode png");
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ui-sketches");
    std::fs::create_dir_all(&dir).expect("create sketch dir");
    let path = dir.join(format!("{name}.png"));
    std::fs::write(&path, png).expect("write png");
    println!("wrote {}", path.display());
}

fn viewport(width: u16, height: u16) -> Rect {
    Rect {
        x: 0,
        y: 0,
        w: width,
        h: height,
    }
}

fn worktree(path: &str, branch: &str, linked: bool, locked: bool) -> WorktreeInfo {
    WorktreeInfo {
        path: path.into(),
        branch: Some(branch.into()),
        detached: false,
        bare: false,
        prunable: false,
        linked,
        lock: locked.then(|| WorktreeLock {
            reason: String::new(),
            stale: false,
        }),
    }
}

// --- workbar-alerts --------------------------------------------------------------------------

fn live_pane(id: u32) -> Pane {
    let mut pane = Pane::new(
        id,
        100,
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 10.0,
        },
    );
    pane.opening = false;
    pane.terminal_active = true;
    pane
}

/// Workspace 1 active and quiet, 2 blocked, 3 finished-unseen: one tab per marker plus an unmarked
/// neighbour, which is what makes "is this subtle enough" answerable at a glance.
fn workbar_backend(phase: bool, calm_phase: bool) -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(viewport(80, 6));
    let state = backend.state_mut();
    state.config.pane.show_workbar = true;
    state.config.workbar.alert.mode = AlertMode::Pulse;
    state.alert_pulse_armed = true;
    state.alert_pulse_phase = phase;
    state.alert_pulse_calm_phase = calm_phase;

    state.current_mut().workspaces[0].panes.push(live_pane(10));
    state.current_mut().workspaces[0].focused_pane = Some(10);
    state.current_mut().focused_pane = Some(10);

    let mut blocked = live_pane(11);
    blocked.terminal.reported_status = Some(rozi::session::protocol::PaneStatus {
        value: "blocked".into(),
        reason: None,
        set_at: 0,
    });
    state.current_mut().workspaces[1].panes.push(blocked);

    let mut finished = live_pane(12);
    finished.terminal.finished_unseen = true;
    state.current_mut().workspaces[2].panes.push(finished);

    backend
}

/// `hover_x` puts the mouse over a tab. Hover is the case worth capturing because the hover style
/// layers over whatever the tab already resolved to: an absolute colour there silently discards the
/// alert, and only a rendered frame shows whether it survived.
fn workbar_capture(name: &str, phase: bool, calm_phase: bool, hover_x: Option<u16>) {
    let mut backend = workbar_backend(phase, calm_phase);
    if let Some(x) = hover_x {
        backend.render();
        backend
            .send_mouse(MouseEvent {
                x,
                y: 0,
                kind: MouseKind::Moved,
                mods: KeyMods::NONE,
            })
            .expect("hover the workbar");
    }
    backend.render();
    write_png(&mut backend, name);
}

fn workbar_alerts() {
    // Blocked runs at the urgent rate and finished at the calm one, so the interesting frames are
    // the three the two rates actually produce together.
    workbar_capture("workbar-alert-both-peak", false, false, None);
    workbar_capture("workbar-alert-urgent-trough", true, false, None);
    workbar_capture("workbar-alert-both-trough", true, true, None);
    // Hovering the blocked tab must lift its colour, not replace it.
    workbar_capture("workbar-alert-hovered", false, false, Some(21));
}

// --- sidebar-worktrees -----------------------------------------------------------------------

/// The shape of a real repository: a primary checkout, a sibling with a running session, a nested
/// agent checkout named after its branch, one whose folder differs from its branch, and a locked
/// one whose session can only be restored.
fn sidebar_worktrees() {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(viewport(100, 26));
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.animations.sidebar = false;
        state.config.sidebar.tabs = vec![SidebarTab::Worktrees];
        state.sidebar.panels[0].tabs = vec![SidebarTabId::new("worktrees")];
        state.sidebar.panels[0].active_tab = Some(SidebarTabId::new("worktrees"));
        let listing = &mut state.sidebar.worktrees;
        listing.source = Some((None, "/home/me/src/rozi".into()));
        listing.loaded = true;
        listing.entries = vec![
            worktree("/home/me/src/rozi", "master", false, false),
            worktree(
                "/home/me/src/rozi-worktrees/feat-login",
                "feat/login",
                true,
                false,
            ),
            worktree(
                "/home/me/src/rozi/.claude/worktrees/extensions-spinner",
                "worktree-extensions-spinner",
                true,
                false,
            ),
            worktree(
                "/home/me/src/rozi/.claude/worktrees/session-fade-duration",
                "fix/config-test-race",
                true,
                false,
            ),
            worktree(
                "/home/me/src/rozi-worktrees/release",
                "release/0.1",
                true,
                true,
            ),
        ];
        let session = |name: &str, running: bool| WorktreeSession {
            name: name.into(),
            running,
        };
        listing.sessions.insert(
            "/home/me/src/rozi-worktrees/feat-login".into(),
            vec![session("wt-feat-login", true)],
        );
        listing
            .sessions
            .insert("/home/me/src/rozi".into(), vec![session("dev", true)]);
        listing.sessions.insert(
            "/home/me/src/rozi-worktrees/release".into(),
            vec![session("release", false)],
        );

        // The pointer on `feat/login` (header, master, then it) shows what Enter would do.
        state.sidebar.panels[0].hovered_row = Some(2);
        state.sidebar.pending_row_close = Some(rozi::state::SidebarClose::Worktree {
            path: "/home/me/src/rozi/.claude/worktrees/extensions-spinner".into(),
            force: false,
        });
    }
    backend.render();
    write_png(&mut backend, "sidebar-worktrees");
}

// --- worktree-picker -------------------------------------------------------------------------

/// The shape of a real repository with agent worktrees nested inside it, one of them carrying a
/// session that can be restored.
fn worktree_picker() {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.state_mut().config.animations.picker = rozi::layout::anim::PickerAnimationStyle::Off;
    let mut picker = WorktreePickerState::new("/home/me/src/rozi".into(), None);
    picker.entries = vec![
        worktree("/home/me/src/rozi", "master", false, false),
        worktree(
            "/home/me/src/rozi/.claude/worktrees/extensions-checking-spinner",
            "worktree-extensions-checking-spinner",
            true,
            true,
        ),
        worktree(
            "/home/me/src/rozi/.claude/worktrees/session-fade-duration",
            "fix/config-test-race",
            true,
            false,
        ),
        worktree(
            "/home/me/src/rozi-worktrees/feat-login",
            "feat/login",
            true,
            false,
        ),
    ];
    picker.sessions.push(DiscoveredSession {
        name: "review".into(),
        origin: SessionOrigin {
            worktree: Some(WorktreeOrigin {
                path: "/home/me/src/rozi-worktrees/feat-login".into(),
            }),
            ..Default::default()
        },
        ephemeral: false,
        host: None,
        remote_target: None,
        status: DiscoveredSessionStatus::Restorable,
    });
    picker.selected = 2;
    backend.state_mut().worktree_picker = Some(picker);

    for (width, height) in [(72, 22), (100, 30), (140, 40)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(&mut backend, &format!("worktree-picker-{width}x{height}"));
    }

    backend.dispatch(Msg::WorktreeNew).expect("open the form");
    {
        let form = backend
            .state_mut()
            .worktree_picker
            .as_mut()
            .and_then(|picker| picker.form.as_mut())
            .expect("new-checkout form");
        form.branch.set_text("feat/login".to_string());
        form.path
            .set_text("/home/me/src/rozi/.worktrees/feat-login".to_string());
        form.unignored = Some(".worktrees".into());
    }
    for (width, height) in [(72, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(&mut backend, &format!("worktree-form-{width}x{height}"));
    }
}

fn floating_title_caps() {
    use rozi::layout::tiling::build_dwindle_tree;
    use rozi::state::PaneTitlebarMode;
    use tui_lipan::prelude::CapStyle;

    for (width, height) in [(48, 12), (80, 24)] {
        for mode in [PaneTitlebarMode::Bar, PaneTitlebarMode::Integrated] {
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(viewport(width, height));
            let state = backend.state_mut();
            state.config.animations.enabled = false;
            state.config.pane.show_workbar = false;
            state.config.pane.titlebar = mode;
            state.config.pane.title_style = CapStyle::Half;
            state.config.pane.padding = (0, 0, 0, 0);
            let workspace = &mut state.current_mut().workspaces[0];
            workspace.panes = vec![live_pane(10)];
            workspace.tile_tree =
                build_dwindle_tree(&[10], workspace.start_axis, &workspace.split_ratios);
            backend.render();
            let output = (1..height - 2)
                .map(|row| {
                    format!(
                        "\x1b[{row};1H\x1b[1;2;3;4;7;9m{}",
                        "dimmed output ".repeat(5)
                    )
                })
                .collect::<String>();
            backend.state_mut().current_mut().workspaces[0].panes[0]
                .terminal
                .process_server_output(output.as_bytes());
            let mut floating = live_pane(11);
            floating.floating = true;
            floating.floating_rect = FloatRect {
                x: 8.0,
                y: 4.0,
                w: 28.0,
                h: 6.0,
            };
            floating.set_custom_title("floating cap");
            backend.state_mut().current_mut().workspaces[0]
                .panes
                .push(floating);
            backend.state_mut().current_mut().focused_pane = Some(11);
            backend.state_mut().current_mut().workspaces[0].focused_pane = Some(11);
            backend.render();
            write_png(
                &mut backend,
                &format!("floating-title-caps-{mode:?}-{width}x{height}"),
            );
        }
    }
}
