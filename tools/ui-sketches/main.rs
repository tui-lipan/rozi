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

const SCENARIOS: [(&str, &str, fn()); 10] = [
    (
        "profile-picker",
        "attached, background, running, inactive, and default profiles",
        profile_picker,
    ),
    (
        "session-picker",
        "All, host browsing, and tab-scoped session search",
        session_picker,
    ),
    (
        "session-picker-no-results",
        "unmatched local and remote search with contextual recovery actions",
        session_picker_no_results,
    ),
    (
        "session-picker-settings",
        "Sessions opening preference and its choice editor",
        session_picker_settings,
    ),
    (
        "agent-picker",
        "agent identity, checkout context, and status at three sizes",
        agent_picker,
    ),
    (
        "worktree-detail-dimming",
        "the real Worktrees sidebar with 0%, 20%, 40%, and 60% detail dimming",
        worktree_detail_dimming,
    ),
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

fn profile_picker() {
    let mut backend = TestBackend::new(AppRoot::default());
    let state = backend.state_mut();
    state.config.animations.picker = rozi::layout::anim::PickerAnimationStyle::Off;
    state.current_mut().session_name = Some("dev".into());
    state.config.profile.default = Some("dev".into());
    let mut picker = rozi::state::ProfilePickerState::new(
        [
            "dev",
            "review",
            "background",
            "scratch",
            "very-long-profile-name-for-another-project",
        ]
        .into_iter()
        .map(|name| rozi::config::ProfileEntry {
            name: name.into(),
            path: format!("/profiles/{name}.toml").into(),
        })
        .collect(),
    );
    picker.running.insert(
        "review".into(),
        DiscoveredSessionStatus::Running {
            panes: 2,
            clients: 0,
            has_layout: true,
        },
    );
    let mut background = rozi::state::Attachment::new();
    background.session_name = Some("background".into());
    background.session_attached = true;
    background.connection = rozi::state::ConnectionState::Connected;
    state.background.insert(1, background);
    state.profile_picker = Some(picker);
    state.show_profile_picker = true;
    if std::env::var_os("TUI_LIPAN_SNAPSHOT_DIAGNOSTIC").is_some() {
        println!(
            "{}",
            backend
                .capture_ui_snapshot_with_options(&tui_lipan::UiSnapshotOptions::diagnostic())
                .to_markdown()
        );
    }
    for (width, height) in [(100, 30), (60, 20)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(&mut backend, &format!("profile-picker-{width}x{height}"));
    }
    backend.state_mut().profile_picker.as_mut().unwrap().input =
        tui_lipan::prelude::TextInput::new("review");
    backend.render();
    write_png(&mut backend, "profile-picker-filtered");
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

fn work_statuses(trees: &[WorktreeInfo]) -> rozi::git::pull_requests::WorktreeStatuses {
    use rozi::git::pull_requests::{PullRequestStatus, WorkStatus, WorktreeStatuses};
    let states = [
        WorkStatus::Passed,
        WorkStatus::Failed,
        WorkStatus::Merged,
        WorkStatus::Running,
        WorkStatus::Open,
        WorkStatus::Draft,
    ];
    WorktreeStatuses {
        checkouts: trees
            .iter()
            .filter(|tree| tree.linked)
            .zip(states)
            .enumerate()
            .map(|(index, (tree, status))| {
                (
                    tree.path.clone(),
                    PullRequestStatus {
                        number: 114 + index as u64,
                        status,
                    },
                )
            })
            .collect(),
        unavailable: false,
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
    let mut backend = sidebar_worktrees_fixture();
    for (width, height) in [(72, 22), (100, 26), (140, 40)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(&mut backend, &format!("sidebar-worktrees-{width}x{height}"));
    }
    {
        let state = backend.state_mut();
        // The pointer on `feat/login` (header, master, then it) shows what Enter would do.
        state.sidebar.panels[0].hovered_row = Some(2);
        state.sidebar.pending_row_close = Some(rozi::state::SidebarClose::Worktree {
            path: "/home/me/src/rozi/.claude/worktrees/extensions-spinner".into(),
            force: false,
        });
    }
    backend.render();
    write_png(&mut backend, "sidebar-worktrees-armed");
}

fn worktree_detail_dimming() {
    let mut backend = sidebar_worktrees_fixture();
    let mut comparisons = Vec::new();
    for (name, amount) in [
        ("current", 0.0),
        ("light", 0.2),
        ("medium", 0.4),
        ("strong", 0.6),
    ] {
        backend.state_mut().sidebar.worktrees.detail_dim_preview = Some(amount);
        for (width, height) in [(72, 22), (100, 26)] {
            backend.set_viewport(viewport(width, height));
            backend.render();
            write_png(
                &mut backend,
                &format!("worktree-detail-{name}-{width}x{height}"),
            );
            if width == 72 {
                comparisons.push((
                    format!("{name} · {:.0}%", amount * 100.0),
                    backend.capture_frame(),
                ));
            }
        }
    }
    // Assemble captured terminal cells, preserving the real sidebar rendering and colours,
    // while leaving out the empty workspace area for a compact visual comparison.
    let panel_width = 31;
    let height = 16;
    let width = (panel_width + 2) * comparisons.len() as u16 - 2;
    let mut blank = comparisons[0].1.cell(1, 1).clone();
    blank.symbol = " ".into();
    let mut frame = tui_lipan::CapturedFrame {
        viewport: viewport(width, height),
        width,
        height,
        cells: vec![blank; usize::from(width * height)],
        cursor: None,
        images: Vec::new(),
    };
    for (index, (label, sidebar)) in comparisons.iter().enumerate() {
        let offset = index as u16 * (panel_width + 2);
        for (x, ch) in label.chars().enumerate() {
            let cell = &mut frame.cells[usize::from(offset) + x];
            cell.symbol = ch.to_string();
            cell.fg = sidebar.cell(2, 3).fg;
        }
        for y in 0..height - 2 {
            for x in 0..panel_width {
                frame.cells[usize::from((y + 2) * width + offset + x)] = sidebar.cell(x, y).clone();
            }
        }
    }
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ui-sketches/worktree-detail-comparison.png");
    std::fs::write(
        &path,
        frame.to_png(&tui_lipan::PngOptions::default()).unwrap(),
    )
    .unwrap();
    println!("wrote {}", path.display());
}

fn sidebar_worktrees_fixture() -> TestBackend<AppRoot> {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.set_viewport(viewport(100, 26));
    {
        let state = backend.state_mut();
        state.sidebar_visible = true;
        state.config.animations.sidebar = false;
        state.config.sidebar.split = false;
        state.sidebar.panels.truncate(1);
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
        listing.statuses = work_statuses(&listing.entries);
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
    }
    backend
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
    picker.entries.extend([
        worktree(
            "/home/me/src/rozi-worktrees/floating-drag",
            "fix/follower-floating-drag-live",
            true,
            false,
        ),
        worktree(
            "/home/me/src/rozi-worktrees/keep-links",
            "fix/keep-links-alive-across-suspend",
            true,
            false,
        ),
        worktree(
            "/home/me/src/rozi-worktrees/sidebar",
            "fix/sidebar-mode-polish",
            true,
            false,
        ),
    ]);
    picker.statuses = work_statuses(&picker.entries);
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

    let (entries, statuses) = {
        let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
        let statuses = std::mem::take(&mut picker.statuses);
        picker.statuses.unavailable = true;
        (picker.entries.clone(), statuses)
    };
    backend.set_viewport(viewport(72, 22));
    backend.render();
    write_png(&mut backend, "worktree-picker-unavailable");
    for (name, error, pending) in [
        ("empty", None, None),
        ("loading", None, Some(999)),
        ("error", Some("Repository unavailable"), None),
    ] {
        let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
        picker.entries.clear();
        picker.pending_list = pending;
        picker.error = error.map(str::to_string);
        backend.render();
        write_png(&mut backend, &format!("worktree-picker-{name}"));
    }
    {
        let picker = backend.state_mut().worktree_picker.as_mut().unwrap();
        picker.entries = entries;
        picker.statuses = statuses;
        picker.error = None;
        picker.pending_list = None;
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

// --- agent-picker ---------------------------------------------------------------------------

fn agent_picker() {
    use rozi::session::protocol::{
        AgentIdentity, AgentRef, DetectedAgent, DetectedAgentState, PaneRef, PublishedRow,
        SessionInstanceId,
    };
    use rozi::state::{AgentLocation, AgentPickerState, AgentPickerTab};

    let mut backend = TestBackend::new(AppRoot::default());
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.sidebar_visible = false;
        state.current_mut().session_name = Some("eph-1234".into());
        let mut pane = Pane::new(
            1,
            100,
            FloatRect {
                x: 0.0,
                y: 0.0,
                w: 80.0,
                h: 24.0,
            },
        );
        pane.terminal.detected_agent = Some(DetectedAgent {
            agent: AgentIdentity::new("claude", "Claude Code").into(),
            state: DetectedAgentState::Working,
        });
        pane.terminal.cwd = Some("/home/me/src/rozi".into());
        pane.terminal.project_root = Some("/home/me/src/rozi".into());
        pane.terminal.git_branch = Some("feat/worktree-work-status".into());
        for (id, title, status) in [
            ("review", "github issue review", "working"),
            ("fix", "fix login redirect", "blocked"),
            ("unicode", "Review 日本語 changes", "idle"),
        ] {
            pane.terminal.published_rows.push(PublishedRow {
                id: id.into(),
                title: title.into(),
                status: status.into(),
                reason: None,
                active: id == "review",
                work_started_at: None,
                cwd: None,
                project: None,
                native_session: None,
            });
            pane.agent_refs.push(AgentRef {
                pane: PaneRef {
                    session_instance: SessionInstanceId::generate(),
                    pane_id: 1,
                    generation: 0,
                },
                slot: Some(id.into()),
                incarnation: pane.agent_refs.len() as u64 + 1,
            });
        }
        state.current_mut().workspaces[0].panes = vec![pane];
        state.agent_picker = Some(AgentPickerState::new(Some(AgentLocation::Here {
            pane: 1,
            row: Some("fix".into()),
        })));
    }
    backend.state_mut().local_agent_snapshot = Some(rozi::session::discovery::LocalAgentSnapshot {
        sessions: Vec::new(),
        agents: vec![rozi::session::protocol::AgentSummary {
            session: "backend".into(),
            pane: 9,
            generation: 0,
            row: None,
            agent: "codex".into(),
            label: "Codex".into(),
            state: "blocked".into(),
            changed_at: 0,
        }],
    });
    for (width, height) in [(64, 22), (100, 30), (180, 40)] {
        backend.set_viewport(viewport(width, height));
        backend.dispatch(Msg::RefreshPaintLayers).unwrap();
        backend.render();
        write_png(&mut backend, &format!("agent-picker-{width}x{height}"));
    }
    backend.state_mut().agent_picker.as_mut().unwrap().tab = AgentPickerTab::Session {
        target: None,
        session: "eph-1234".into(),
    };
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.dispatch(Msg::RefreshPaintLayers).unwrap();
        backend.render();
        write_png(
            &mut backend,
            &format!("agent-picker-session-{width}x{height}"),
        );
    }
    backend.state_mut().agent_picker.as_mut().unwrap().tab = AgentPickerTab::Session {
        target: None,
        session: "backend".into(),
    };
    backend.dispatch(Msg::RefreshPaintLayers).unwrap();
    backend.render();
    write_png(&mut backend, "agent-picker-other-session");
    backend.state_mut().agent_picker.as_mut().unwrap().tab = AgentPickerTab::All;
    backend.set_viewport(viewport(180, 40));
    backend.state_mut().current_mut().workspaces[0].panes[0].terminal.published_rows[0].title =
        "Investigate a very long activity title that exceeds the available terminal width and must truncate cleanly".into();
    backend.dispatch(Msg::RefreshPaintLayers).unwrap();
    backend.render();
    write_png(&mut backend, "agent-picker-overflow");
    backend.state_mut().local_agent_snapshot = None;
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.dispatch(Msg::RefreshPaintLayers).unwrap();
        backend.render();
        write_png(
            &mut backend,
            &format!("agent-picker-single-session-{width}x{height}"),
        );
    }
    backend.state_mut().current_mut().workspaces[0]
        .panes
        .clear();
    backend.dispatch(Msg::RefreshPaintLayers).unwrap();
    backend.render();
    write_png(&mut backend, "agent-picker-empty");
}

fn session_picker() {
    use rozi::session::remote::RemoteTarget;
    use rozi::state::{SessionPickerState, SessionPickerTab};
    let mut backend = TestBackend::new(AppRoot::default());
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        state.show_session_picker = true;
        state.session_picker = Some(SessionPickerState::new(
            [
                ("dev", None),
                ("dev", Some("workbox")),
                ("backend", Some("buildbox")),
            ]
            .into_iter()
            .map(|(name, host)| DiscoveredSession {
                name: name.into(),
                host: host.map(str::to_string),
                remote_target: host.map(|host| RemoteTarget::Alias(host.into())),
                origin: Default::default(),
                ephemeral: false,
                status: DiscoveredSessionStatus::Running {
                    panes: 3,
                    clients: 1,
                    has_layout: true,
                },
            })
            .collect(),
        ));
    }
    for (name, tab, query) in [
        ("all", SessionPickerTab::All, ""),
        (
            "host",
            SessionPickerTab::Host(Some(RemoteTarget::Alias("workbox".into()))),
            "",
        ),
        ("local-search", SessionPickerTab::Host(None), "dev"),
        ("all-search", SessionPickerTab::All, "dev@workbox"),
        (
            "host-search",
            SessionPickerTab::Host(Some(RemoteTarget::Alias("workbox".into()))),
            "dev",
        ),
        (
            "local-search-empty",
            SessionPickerTab::Host(None),
            "dev@workbox",
        ),
    ] {
        let picker = backend.state_mut().session_picker.as_mut().unwrap();
        picker.tab = tab;
        picker.input.set_text("");
        picker.keep_selection_in_tab();
        backend
            .update_level(Msg::SessionPickerQueryChanged(query.into()))
            .unwrap();
        for (width, height) in [(64, 22), (100, 30)] {
            backend.set_viewport(viewport(width, height));
            backend.render();
            write_png(
                &mut backend,
                &format!("session-picker-{name}-{width}x{height}"),
            );
        }
    }
    let picker = backend.state_mut().session_picker.as_mut().unwrap();
    let mut url = picker.entries[1].clone();
    url.remote_target = Some(RemoteTarget::Url {
        user: None,
        host: "workbox".into(),
        port: None,
    });
    picker.entries.retain(|entry| entry.name == "dev");
    picker.entries.push(url);
    picker.selected = 0;
    picker.tab = SessionPickerTab::All;
    picker.input.set_text(String::new());
    backend.state_mut().show_session_picker = false;
    backend.render();
    backend.state_mut().show_session_picker = true;
    backend.set_viewport(viewport(120, 30));
    backend.render();
    write_png(&mut backend, "session-picker-colliding-targets");
    let picker = backend.state_mut().session_picker.as_mut().unwrap();
    picker.entries.retain(|entry| entry.remote_target.is_none());
    picker.tab = SessionPickerTab::Host(None);
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(
            &mut backend,
            &format!("session-picker-local-{width}x{height}"),
        );
    }
    // Exercise the actual host-management route into the shared tabbed picker.
    let target = RemoteTarget::Alias("workbox".into());
    let mut remote = backend.state().session_picker.as_ref().unwrap().entries[0].clone();
    remote.remote_target = Some(target.clone());
    remote.host = Some(target.display_label());
    backend.state_mut().remote.added_hosts.push(target.clone());
    backend.dispatch(Msg::SessionPickerRemoteHosts).unwrap();
    backend.state_mut().command_link = None;
    let picker = backend.state_mut().remote_picker.as_mut().unwrap();
    picker.probe_epoch = 1;
    picker.host_probe = rozi::state::HostProbe::InFlight;
    picker.probe_target = Some(target.clone());
    backend
        .update_level(Msg::RemoteHostSessionsDiscovered {
            epoch: 1,
            target: target.clone(),
            rows: Ok(vec![remote]),
        })
        .unwrap();
    backend
        .update_level(Msg::RemotePickerHostActivate(target))
        .unwrap();
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(
            &mut backend,
            &format!("session-picker-from-host-{width}x{height}"),
        );
    }
}

fn session_picker_settings() {
    let mut backend = TestBackend::new(AppRoot::default());
    backend.state_mut().config.animations.enabled = false;
    backend.state_mut().show_session_picker = false;
    backend.state_mut().session_picker = None;
    backend.state_mut().show_settings = true;
    backend.state_mut().settings_navigation.tab = rozi::state::SettingsTab::Sessions;
    backend.state_mut().settings_selected =
        Some(rozi::state::SettingsAction::CycleSessionPickerOpenOn);
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(
            &mut backend,
            &format!("session-picker-settings-{width}x{height}"),
        );
    }
    backend
        .update_level(Msg::SettingsActivate(
            rozi::state::SettingsAction::CycleSessionPickerOpenOn,
        ))
        .unwrap();
    for (width, height) in [(64, 22), (100, 30)] {
        backend.set_viewport(viewport(width, height));
        backend.render();
        write_png(
            &mut backend,
            &format!("session-picker-opening-choice-{width}x{height}"),
        );
    }
}

fn session_picker_no_results() {
    let mut backend = TestBackend::new(AppRoot::default());
    {
        let state = backend.state_mut();
        state.config.animations.enabled = false;
        *state.current_mut() = rozi::state::Attachment::new();
        state.current_mut().session_name = Some(rozi::state::ephemeral_session_name());
        state.current_mut().session_attached = true;
        state.show_session_picker = true;
    }
    for (name, target) in [
        ("local", None),
        (
            "host",
            Some(rozi::session::remote::RemoteTarget::Alias("workbox".into())),
        ),
    ] {
        let mut picker = rozi::state::SessionPickerState::new(Vec::new()).on_tab(target);
        picker.input.set_text("Efefef");
        backend.state_mut().session_picker = Some(picker);
        for (width, height) in [(64, 22), (100, 30)] {
            backend.set_viewport(viewport(width, height));
            backend.render();
            write_png(
                &mut backend,
                &format!("session-picker-no-results-{name}-{width}x{height}"),
            );
        }
    }
}
