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

/// The same pane, plus the Git project the session server resolved for its cwd.
fn agent_pane_in_project(
    id: PaneId,
    kind: AgentKind,
    status: Option<(&str, Option<&str>)>,
    cwd: &str,
    root: &str,
    branch: &str,
) -> Pane {
    let mut pane = agent_pane(id, kind, status, Some(cwd));
    pane.terminal.project_root = Some(root.to_string());
    pane.terminal.git_branch = Some(branch.to_string());
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
                state.current_mut().workspaces[0].panes = vec![
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

/// Agents spread through one repository read as one project headed by its branch, with each row
/// saying where in the project it sits.
#[test]
fn agents_tab_heads_projects_with_their_branch() {
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
                state.current_mut().workspaces[0].panes = vec![
                    agent_pane_in_project(
                        1,
                        AgentKind::Claude,
                        Some(("working", Some("wiring the sidebar"))),
                        "/home/x/hyprmux",
                        "/home/x/hyprmux",
                        "master",
                    ),
                    agent_pane_in_project(
                        2,
                        AgentKind::Codex,
                        Some(("idle", None)),
                        "/home/x/hyprmux/src/view",
                        "/home/x/hyprmux",
                        "master",
                    ),
                    // A second checkout of the same repository: its own directory, its own branch.
                    agent_pane_in_project(
                        3,
                        AgentKind::OpenCode,
                        Some(("blocked", Some("needs approval"))),
                        "/home/x/hyprmux-wt",
                        "/home/x/hyprmux-wt",
                        "feat/agent-branches",
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
                    .unwrap_or_else(|| panic!("sidebar shows {needle:?}\n{}", sidebar.join("\n")))
            };

            // One project, not one per directory: the nested agent is under the `hyprmux` header.
            let hyprmux = line_index("hyprmux ");
            assert!(hyprmux < line_index("Claude Code"));
            assert!(hyprmux < line_index("Codex"));
            // The branch heads its own group, right-aligned on the header line. `31` drops the
            // panel border the 32-column capture includes.
            let content = |line: &str| line.chars().take(31).collect::<String>();
            let right_edge = |line: &str| content(line).trim_end().chars().count();
            assert!(content(&sidebar[hyprmux]).trim_end().ends_with("master"));
            // It shares the badge column with the rows below it, so the two read as one rail.
            assert_eq!(
                right_edge(&sidebar[hyprmux]),
                right_edge(&sidebar[hyprmux + 1])
            );
            // What grouping on the root gave up comes back per row.
            assert!(sidebar.iter().any(|line| line.contains("src/view · 1")));
            // A worktree is a separate group, and the branch is what tells the two apart.
            let worktree = line_index("hyprmux-wt");
            // Too long for half the header, so it keeps the tail — the end of a branch name is
            // what distinguishes it from its neighbours.
            assert!(sidebar[worktree].contains("…/agent-branches"));
            assert!(worktree < line_index("OpenCode"));
        })
        .expect("spawn agents branch smoke thread")
        .join()
        .expect("agents branch smoke completes");
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
                state.current_mut().workspaces[0].panes = vec![finished];
            }
            // Looking at the pane through any focus path acknowledges the finish.
            backend.dispatch(Msg::SidebarFocusPane(7)).ok();
            let pane = &backend.state().current().workspaces[0].panes[0];
            assert_eq!(backend.state().current().focused_pane, Some(7));
            assert!(!pane.terminal.finished_unseen);
        })
        .expect("spawn focus-clears-pulse thread")
        .join()
        .expect("focus-clears-pulse completes");
}
