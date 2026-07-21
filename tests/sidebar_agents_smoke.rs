use hyprmux::config::SidebarTab;
use hyprmux::session::protocol::{AgentKind, DetectedAgent, DetectedAgentState, PaneStatus};
use hyprmux::state::{Pane, PaneId};
use hyprmux::{HyprmuxApp, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

fn agent_pane(
    id: PaneId,
    kind: AgentKind,
    status: Option<(&str, Option<&str>)>,
    cwd: Option<&str>,
) -> Pane {
    let mut pane = Pane::new(
        id,
        100,
        FloatRect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 10.0,
        },
    );
    pane.terminal.detected_agent = Some(DetectedAgent {
        kind,
        state: DetectedAgentState::Idle,
    });
    // Dated now, so the rendered elapsed time is a plausible small value rather than the decades
    // an epoch-anchored stamp would produce.
    let set_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    pane.terminal.reported_status = status.map(|(value, reason)| PaneStatus {
        value: value.to_string(),
        reason: reason.map(str::to_string),
        set_at,
    });
    pane.terminal.cwd = cwd.map(str::to_string);
    pane
}

#[test]
fn agents_tab_renders_project_groups() {
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
                state.config.sidebar.tabs = vec![SidebarTab::Agents];
                state.sidebar.active_tab = Some(SidebarTab::Agents.id());
                let mut finished = agent_pane(
                    2,
                    AgentKind::OpenCode,
                    Some(("idle", None)),
                    Some("/home/x/work/hyprmux"),
                );
                finished.terminal.finished_unseen = true;
                let mut done = agent_pane(
                    5,
                    AgentKind::Gemini,
                    Some(("done", None)),
                    Some("/home/x/oss/tools"),
                );
                done.terminal.finished_unseen = true;
                state.workspaces[0].panes = vec![
                    agent_pane(
                        1,
                        AgentKind::Claude,
                        Some(("blocked", Some("needs approval"))),
                        Some("/home/x/work/hyprmux"),
                    ),
                    finished,
                    agent_pane(
                        3,
                        AgentKind::Codex,
                        Some(("working", None)),
                        Some("/home/x/oss/api"),
                    ),
                    done,
                    agent_pane(4, AgentKind::Aider, None, None),
                    agent_pane(
                        6,
                        AgentKind::OpenCode,
                        Some(("compacting", None)),
                        Some("/home/x/oss/tools"),
                    ),
                ];
            }
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();
            let sidebar: Vec<String> = lines
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect();
            let line_index = |needle: &str| {
                sidebar
                    .iter()
                    .position(|line| line.contains(needle))
                    .unwrap_or_else(|| panic!("sidebar shows {needle:?}"))
            };
            // Alphabetical group order with the unknown-cwd group last; each project header
            // precedes its agent rows.
            let api = line_index("api");
            let hyprmux_group = line_index("hyprmux");
            let tools = line_index("tools");
            let elsewhere = line_index("elsewhere");
            assert!(api < hyprmux_group && hyprmux_group < tools && tools < elsewhere);
            assert!(api < line_index("Codex"));
            assert!(hyprmux_group < line_index("Claude Code"));
            assert!(elsewhere < line_index("Aider"));
            // The detail line spends its width on what the agent is doing and how long it has been
            // at it, not on repeating a status the glyph column already carries.
            assert!(sidebar.iter().any(|line| line.contains("needs approval")));
            assert!(sidebar.iter().any(|line| line.contains("0s")));
            assert!(!sidebar.iter().any(|line| line.contains("blocked")));
            // An agent-defined status has no glyph of its own, so its word survives.
            assert!(sidebar.iter().any(|line| line.contains("compacting")));
            // A finished-unseen agent shows the filled attention pulse.
            assert!(sidebar.iter().any(|line| line.contains('●')));
        })
        .expect("spawn agents sidebar smoke thread")
        .join()
        .expect("agents sidebar smoke completes");
}

#[test]
fn focusing_a_finished_agent_clears_its_pulse() {
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
                let mut finished = agent_pane(
                    7,
                    AgentKind::Claude,
                    Some(("idle", None)),
                    Some("/home/x/repo"),
                );
                finished.terminal.finished_unseen = true;
                state.workspaces[0].panes = vec![finished];
            }
            // Looking at the pane through any focus path acknowledges the finish.
            backend.dispatch(Msg::SidebarFocusPane(7)).ok();
            let pane = &backend.state().workspaces[0].panes[0];
            assert_eq!(backend.state().focused_pane, Some(7));
            assert!(!pane.terminal.finished_unseen);
        })
        .expect("spawn focus-clears-pulse thread")
        .join()
        .expect("focus-clears-pulse completes");
}
