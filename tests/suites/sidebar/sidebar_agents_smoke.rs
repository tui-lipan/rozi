use rozi::config::SidebarTab;
use rozi::session::protocol::{AgentIdentity, DetectedAgent, DetectedAgentState, PaneStatus};

/// One detected agent's public identity, as a definition in the built-in catalog declares it.
fn agent(id: &str, label: &str) -> AgentIdentity {
    AgentIdentity::new(id, label)
}
use rozi::state::{Pane, PaneId};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;
use tui_lipan::prelude::{FloatRect, Rect};

fn agent_pane(
    id: PaneId,
    agent: AgentIdentity,
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
        agent: agent.into(),
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

fn published_row(
    id: &str,
    title: &str,
    status: &str,
    reason: Option<&str>,
    active: bool,
) -> rozi::session::protocol::PublishedRow {
    rozi::session::protocol::PublishedRow {
        id: id.to_string(),
        title: title.to_string(),
        status: status.to_string(),
        reason: reason.map(str::to_string),
        active,
        // Dated now, so a rendered elapsed time is a small plausible value.
        work_started_at: Some(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
        ),
    }
}

/// Give the Activity tab the whole sidebar: these assertions are about grouping and row order, so
/// the default two-panel split would only halve the rows they can see.
fn agents_fill_the_sidebar(state: &mut rozi::state::State) {
    state.sidebar_visible = true;
    // Revealing the sidebar after the first frame is a real toggle, so it runs the real
    // slide. These tests assert on the settled column, not on a frame part-way through it.
    state.config.animations.sidebar = false;
    state.config.sidebar.tabs = vec![SidebarTab::Activity];
    state.config.sidebar.split = false;
    state.sidebar.apply_configured_panels(&state.config.sidebar);
    state.sidebar.panels[0].active_tab = Some(SidebarTab::Activity.id());
}

/// The same pane, plus the Git project the session server resolved for its cwd.
fn agent_pane_in_project(
    id: PaneId,
    agent: AgentIdentity,
    status: Option<(&str, Option<&str>)>,
    cwd: &str,
    root: &str,
    branch: &str,
) -> Pane {
    let mut pane = agent_pane(id, agent, status, Some(cwd));
    pane.terminal.project_root = Some(root.to_string());
    pane.terminal.git_branch = Some(branch.to_string());
    pane
}

#[test]
fn agents_tab_renders_project_groups() {
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
                agents_fill_the_sidebar(state);
                let mut finished = agent_pane(
                    2,
                    agent("opencode", "OpenCode"),
                    Some(("idle", None)),
                    Some("/home/x/work/rozi"),
                );
                finished.terminal.finished_unseen = true;
                let mut done = agent_pane(
                    5,
                    agent("gemini", "Gemini CLI"),
                    Some(("done", None)),
                    Some("/home/x/oss/tools"),
                );
                done.terminal.finished_unseen = true;
                state.current_mut().workspaces[0].panes = vec![
                    agent_pane(
                        1,
                        agent("claude", "Claude Code"),
                        Some(("blocked", Some("needs approval"))),
                        Some("/home/x/work/rozi"),
                    ),
                    finished,
                    agent_pane(
                        3,
                        agent("codex", "Codex"),
                        Some(("working", None)),
                        Some("/home/x/oss/api"),
                    ),
                    done,
                    agent_pane(4, agent("aider", "Aider"), None, None),
                    agent_pane(
                        6,
                        agent("opencode", "OpenCode"),
                        Some(("compacting", None)),
                        Some("/home/x/oss/tools"),
                    ),
                ];
            }
            let targets: Vec<_> = backend
                .state()
                .sidebar_item_projections(&SidebarTab::Activity)
                .into_iter()
                .filter_map(|item| match item.target {
                    rozi::state::RowTarget::Pane(id) => Some(id),
                    _ => None,
                })
                .collect();
            assert_eq!(targets, vec![3, 1, 2, 6, 5, 4]);
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
            let rozi_group = line_index("rozi");
            let tools = line_index("tools");
            let elsewhere = line_index("elsewhere");
            assert!(api < rozi_group && rozi_group < tools && tools < elsewhere);
            assert!(api < line_index("Codex"));
            assert!(rozi_group < line_index("Claude Code"));
            assert!(elsewhere < line_index("Aider"));
            // The detail line spends its width on what the agent is doing and how long it has been
            // at it, not on repeating a status the glyph column already carries.
            assert!(sidebar.iter().any(|line| line.contains("needs approval")));
            assert!(sidebar.iter().any(|line| line.contains("0s")));
            assert!(
                !sidebar
                    .iter()
                    .any(|line| line.to_ascii_lowercase().contains("blocked"))
            );
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
            let mut backend = TestBackend::new(AppRoot::default());
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 100,
                h: 30,
            });
            {
                let state = backend.state_mut();
                agents_fill_the_sidebar(state);
                state.current_mut().workspaces[0].panes = vec![
                    agent_pane_in_project(
                        1,
                        agent("claude", "Claude Code"),
                        Some(("working", Some("wiring the sidebar"))),
                        "/home/x/rozi",
                        "/home/x/rozi",
                        "master",
                    ),
                    agent_pane_in_project(
                        2,
                        agent("codex", "Codex"),
                        Some(("idle", None)),
                        "/home/x/rozi/src/view",
                        "/home/x/rozi",
                        "master",
                    ),
                    // A second checkout of the same repository: its own directory, its own branch.
                    agent_pane_in_project(
                        3,
                        agent("opencode", "OpenCode"),
                        Some(("blocked", Some("needs approval"))),
                        "/home/x/rozi-wt",
                        "/home/x/rozi-wt",
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

            // One project, not one per directory: the nested agent is under the `rozi` header.
            let rozi = line_index("rozi ");
            assert!(rozi < line_index("Claude Code"));
            assert!(rozi < line_index("Codex"));
            // The branch heads its own group, right-aligned on the header line. `31` drops the
            // panel border the 32-column capture includes.
            let content = |line: &str| line.chars().take(31).collect::<String>();
            let right_edge = |line: &str| content(line).trim_end().chars().count();
            assert!(content(&sidebar[rozi]).trim_end().ends_with("master"));
            // It shares the badge column with the rows below it, so the two read as one rail.
            assert_eq!(right_edge(&sidebar[rozi]), right_edge(&sidebar[rozi + 1]));
            // What grouping on the root gave up comes back per row.
            assert!(sidebar.iter().any(|line| line.contains("src/view · 1")));
            // A worktree is a separate group, and the branch is what tells the two apart.
            let worktree = line_index("rozi-wt");
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
            let mut backend = TestBackend::new(AppRoot::default());
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
                    agent("claude", "Claude Code"),
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

/// A pane that publishes its own agents renders one row each, named after the agent and numbered
/// so the rows are distinguishable, with each slot's own title as its activity.
#[test]
fn published_slots_render_one_numbered_row_each() {
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
                agents_fill_the_sidebar(state);
                let mut publisher = agent_pane(
                    1,
                    agent("opencode", "OpenCode"),
                    None,
                    Some("/home/x/work/rozi"),
                );
                publisher.terminal.published_rows = vec![
                    published_row("ses_a", "audit the widget layer", "working", None, true),
                    published_row(
                        "ses_b",
                        "fix the flaky test",
                        "blocked",
                        Some("permission required"),
                        false,
                    ),
                    published_row("ses_c", "update the changelog", "idle", None, false),
                    // Titled after the agent itself, so it has no activity to show and its
                    // detail line falls back to the state word.
                    published_row("ses_d", "OpenCode", "idle", None, false),
                    // A fresh session, blocked on its first question before anything has titled
                    // it: the reason is all this row has to say until a title arrives.
                    published_row("ses_e", "", "blocked", Some("answer required"), false),
                ];
                state.current_mut().workspaces[0].panes = vec![publisher];
            }
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();
            let sidebar: Vec<String> = lines
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect();
            for line in &sidebar {
                println!("{line}");
            }

            // The name column keeps the agent and adds the slot's position.
            for name in ["OpenCode #1", "OpenCode #2", "OpenCode #3"] {
                assert!(
                    sidebar.iter().any(|line| line.contains(name)),
                    "sidebar shows {name:?}"
                );
            }
            // A slot's own title is its activity, and it outranks a reason: the name column
            // cannot say which tab this is, so a prompt must not hide the one thing that can.
            assert!(sidebar.iter().any(|line| line.contains("audit the widget")));
            assert!(sidebar.iter().any(|line| line.contains("fix the flaky")));
            assert!(
                !sidebar
                    .iter()
                    .any(|line| line.contains("permission required")),
                "a titled slot shows its title, not its reason"
            );
            // Until a title exists, the reason is all the row has.
            assert!(sidebar.iter().any(|line| line.contains("answer required")));
            // An id is an opaque handle and must never stand in for a missing title.
            assert!(!sidebar.iter().any(|line| line.contains("ses_")));
            // A slot with nothing to say about its work falls back to the state word, which is
            // capitalized like the rest of the chrome rather than echoing the wire value.
            assert!(
                sidebar.iter().any(|line| line.contains("Idle")),
                "a slot with no activity shows its state word"
            );
            assert!(
                !sidebar.iter().any(|line| line.contains("idle")),
                "the lowercase wire value must not reach the screen"
            );
        })
        .expect("spawn published slots render thread")
        .join()
        .expect("published slots render completes");
}

#[test]
fn published_rows_render_without_detected_agent() {
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
                agents_fill_the_sidebar(state);
                let mut publisher = Pane::new(
                    1,
                    100,
                    FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 20.0,
                        h: 10.0,
                    },
                );
                publisher.terminal.detected_agent = None;
                publisher.terminal.cwd = Some("/home/x/work/rozi".into());
                publisher.terminal.published_rows = vec![
                    published_row("build", "Cargo Watch", "working", Some("compiling"), true),
                    published_row("test", "", "blocked", Some("test failure"), false),
                ];
                state.current_mut().workspaces[0].panes = vec![publisher];
            }
            backend.render();
            let lines = backend.capture_frame().to_fixed_grid_lines();
            let sidebar: Vec<String> = lines
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect();
            for line in &sidebar {
                println!("{line}");
            }

            assert!(sidebar.iter().any(|line| line.contains("Cargo Watch")));
            assert!(sidebar.iter().any(|line| line.contains("shell")));
            assert!(sidebar.iter().any(|line| line.contains("compiling")));
            assert!(sidebar.iter().any(|line| line.contains("test failure")));
        })
        .expect("spawn thread")
        .join()
        .expect("thread completes");
}
