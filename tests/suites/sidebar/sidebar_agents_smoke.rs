use rozi::config::SidebarTab;
use rozi::session::protocol::{
    AgentIdentity, AgentRef, DetectedAgent, DetectedAgentState, PaneRef, PaneStatus,
    SessionInstanceId,
};

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
    pane.agents = vec![agent_ref(id, None, 1).into()];
    pane.terminal.cwd = cwd.map(str::to_string);
    pane
}

/// Give each of `pane`'s agents one finished run this client has not looked at.
fn finish_unseen(state: &mut rozi::state::State, pane: &mut rozi::state::Pane) {
    state
        .agent_attention
        .observe_records(&pane.agents, rozi::state::FirstSighting::Baseline);
    for record in &mut pane.agents {
        record.completions += 1;
    }
}

fn agent_ref(pane_id: PaneId, slot: Option<&str>, incarnation: u64) -> AgentRef {
    AgentRef {
        pane: PaneRef {
            session_instance: SessionInstanceId::generate(),
            pane_id,
            generation: 0,
        },
        slot: slot.map(|id| rozi::session::protocol::RowKey::new("sessions", id)),
        incarnation,
    }
}

fn published_row(
    id: &str,
    title: &str,
    status: &str,
    reason: Option<&str>,
    active: bool,
) -> rozi::session::protocol::PublishedRow {
    rozi::session::protocol::PublishedRow {
        publisher: "sessions".into(),
        kind: rozi::session::protocol::PublisherKind::Agent,
        live_session: None,
        reported: false,
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
        cwd: None,
        project: None,
        native_session: None,
    }
}

/// Give the Activity tab the whole sidebar: these assertions are about grouping and row order, so
/// the default two-panel split would only halve the rows they can see.
fn agents_fill_the_sidebar(state: &mut rozi::state::State) {
    state.sidebar.shown = [true, false];
    // Revealing the sidebar after the first frame is a real toggle, so it runs the real
    // slide. These tests assert on the settled column, not on a frame part-way through it.
    state.config.animations.sidebar = false;
    state.config.sidebar.tabs = vec![SidebarTab::activity()];
    state.config.sidebar.layout.left.panel_count = 1;
    state.sidebar.apply_configured_panels(&state.config.sidebar);
    state.sidebar.panels[0].active_tab = Some(SidebarTab::activity().id());
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
    rozi::test_support::on_large_stack(|| {
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
            finish_unseen(state, &mut finished);
            let mut done = agent_pane(
                5,
                agent("gemini", "Gemini CLI"),
                Some(("done", None)),
                Some("/home/x/oss/tools"),
            );
            finish_unseen(state, &mut done);
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
            .sidebar_targets(&SidebarTab::activity())
            .into_iter()
            .filter_map(|target| match target {
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
        let elsewhere = line_index("Elsewhere");
        assert!(api < rozi_group && rozi_group < tools && tools < elsewhere);
        assert!(api < line_index("Codex"));
        assert!(rozi_group < line_index("Claude Code"));
        assert!(elsewhere < line_index("Aider"));
        // The detail line spends its width on what the agent is doing and how long it has been
        // at it, not on repeating a status the glyph column already carries.
        assert!(sidebar.iter().any(|line| line.contains("needs approval")));
        // Dated `now` at pane construction, so a slow runner can tick past 0s before paint.
        // The check is that a duration is on the detail line, not that this suite finished
        // within one second.
        assert!(sidebar.iter().any(|line| {
            line.split_whitespace().any(|token| {
                let digits = token.strip_suffix('s').unwrap_or("");
                !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit())
            })
        }));
        assert!(
            !sidebar
                .iter()
                .any(|line| line.to_ascii_lowercase().contains("blocked"))
        );
        // An agent-defined status has no glyph of its own, so its word survives.
        assert!(sidebar.iter().any(|line| line.contains("compacting")));
        // A finished-unseen agent shows the filled attention pulse.
        assert!(sidebar.iter().any(|line| line.contains('●')));
    });
}

/// Agents spread through one repository read as one project headed by its branch, with each row
/// saying where in the project it sits.
/// Two worktrees of one repository carry the same name, so in a narrow sidebar the branch is what
/// must survive: it keeps its width and its tail, and the name gives way. A repository with one
/// checkout keeps its name whole instead.
#[test]
fn worktree_headings_keep_their_branch_when_the_name_repeats() {
    rozi::test_support::on_large_stack(|| {
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
            state.config.sidebar.layout.left.width = 28;
            let repo = "/x/very-long-repository-name";
            let primary = agent_pane_in_project(
                1,
                agent("claude", "Claude Code"),
                Some(("working", None)),
                repo,
                repo,
                "master",
            );
            let worktree_root = "/x/very-long-repository-name/.worktrees/inbox";
            let mut worktree = agent_pane_in_project(
                2,
                agent("claude", "Claude Code"),
                Some(("working", None)),
                worktree_root,
                worktree_root,
                "feat/worktree-work-status",
            );
            worktree.terminal.repository = Some(repo.to_string());
            let lipan = agent_pane_in_project(
                3,
                agent("codex", "Codex"),
                Some(("working", None)),
                "/x/tui-lipan",
                "/x/tui-lipan",
                "feat/an-equally-long-branch-name",
            );
            state.current_mut().workspaces[0].panes = vec![primary, worktree, lipan];
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let sidebar: Vec<String> = lines
            .iter()
            .map(|line| line.chars().take(29).collect())
            .collect();
        let heading = |needle: &str| {
            sidebar
                .iter()
                .find(|line| line.contains(needle))
                .unwrap_or_else(|| panic!("{needle:?} in {sidebar:#?}"))
                .clone()
        };
        assert!(heading("master").contains("very-"), "{sidebar:#?}");
        let worktree = heading("work-status");
        assert!(
            worktree.contains(" ve…"),
            "the name keeps a stub where the branch needs the room: {sidebar:#?}"
        );
        assert!(
            worktree.contains("…")
                && worktree
                    .trim_end_matches([' ', '│'])
                    .ends_with("work-status"),
            "the long branch keeps its tail: {sidebar:#?}"
        );
        assert!(
            heading("tui-lipan").contains("tui-lipan"),
            "a lone checkout keeps its name: {sidebar:#?}"
        );
    });
}

/// With `worktrees = "repository"` a repository is named once and each checkout is a line beneath
/// it, its branch keeping its end where the line must shorten. A repository with one checkout keeps
/// its usual heading.
#[test]
fn the_repository_layout_names_a_repository_once_with_a_line_per_checkout() {
    rozi::test_support::on_large_stack(|| {
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
            state.config.sidebar.tabs = vec![SidebarTab::Activity {
                config: rozi::config::SidebarActivityConfig {
                    worktrees: rozi::config::ActivityWorktrees::Repository,
                },
            }];
            state.config.sidebar.layout.left.width = 28;
            let repo = "/x/rozi";
            let primary = agent_pane_in_project(
                1,
                agent("claude", "Claude Code"),
                Some(("working", None)),
                repo,
                repo,
                "master",
            );
            let root = "/x/rozi/.worktrees/inbox";
            let mut worktree = agent_pane_in_project(
                2,
                agent("codex", "Codex"),
                Some(("working", None)),
                root,
                root,
                "feat/an-agent-attention-inbox-branch",
            );
            worktree.terminal.repository = Some(repo.to_string());
            let lone = agent_pane_in_project(
                3,
                agent("codex", "Codex"),
                Some(("working", None)),
                "/x/lipan",
                "/x/lipan",
                "main",
            );
            state.current_mut().workspaces[0].panes = vec![primary, worktree, lone];
        }
        backend.render();
        let sidebar: Vec<String> = backend
            .capture_frame()
            .to_fixed_grid_lines()
            .iter()
            .map(|line| line.chars().take(29).collect())
            .collect();
        let named = sidebar
            .iter()
            .filter(|line| line.trim_start().starts_with("rozi"))
            .count();
        assert_eq!(named, 1, "the repository is named once: {sidebar:#?}");
        let master = sidebar
            .iter()
            .position(|line| line.contains("master"))
            .expect("a line for the primary checkout");
        let branch = sidebar
            .iter()
            .position(|line| line.contains("inbox-branch"))
            .unwrap_or_else(|| panic!("the branch keeps its end: {sidebar:#?}"));
        assert!(sidebar[branch].contains('…'), "{sidebar:#?}");
        assert!(
            sidebar[master + 1].contains("Claude Code") && sidebar[branch + 1].contains("Codex"),
            "each checkout's agents sit beneath its line: {sidebar:#?}"
        );
        // A repository with one checkout keeps its usual heading, branch beside the name.
        let lone = sidebar
            .iter()
            .find(|line| line.contains("lipan"))
            .unwrap_or_else(|| panic!("the lone repository: {sidebar:#?}"));
        assert!(lone.contains("main"), "{sidebar:#?}");
        assert!(
            !sidebar
                .iter()
                .any(|line| line.trim_start().starts_with("└ main")),
            "no branch line of its own: {sidebar:#?}"
        );
    });
}

#[test]
fn agents_tab_heads_projects_with_their_branch() {
    rozi::test_support::on_large_stack(|| {
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
        // The branch takes whatever width the project name leaves rather than a fixed share of
        // the header, held off the name by a three-cell gap.
        let header = &sidebar[worktree];
        assert!(header.contains("rozi-wt   ") && header.contains("feat/agent-branches"));
        assert!(worktree < line_index("OpenCode"));
    });
}

#[test]
fn focusing_a_finished_agent_clears_its_pulse() {
    rozi::test_support::on_large_stack(|| {
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
            finish_unseen(state, &mut finished);
            state.current_mut().workspaces[0].panes = vec![finished];
        }
        // Looking at the pane through any focus path acknowledges the finish.
        backend.dispatch(Msg::SidebarFocusPane(7)).ok();
        let pane = &backend.state().current().workspaces[0].panes[0];
        assert_eq!(backend.state().current().focused_pane, Some(7));
        assert!(!pane.finished_unseen(&backend.state().agent_attention));
    });
}

/// A pane that publishes its own agents renders one row each, named after the agent and numbered
/// so the rows are distinguishable, with each slot's own title as its activity.
#[test]
fn published_sessions_render_under_one_cluster() {
    rozi::test_support::on_large_stack(|| {
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
            publisher.agents = ["ses_a", "ses_b", "ses_c", "ses_d", "ses_e"]
                .into_iter()
                .enumerate()
                .map(|(index, slot)| agent_ref(publisher.id, Some(slot), index as u64 + 1).into())
                .collect();
            state.current_mut().workspaces[0].panes = vec![publisher];
        }
        backend.render();
        let lines = backend.capture_frame().to_fixed_grid_lines();
        let sidebar: Vec<String> = lines
            .iter()
            .map(|line| line.chars().take(32).collect())
            .collect();

        // One cluster row names the program, and its sessions are listed beneath it. Untitled
        // sessions fall back to the program's name, numbered so the two read apart.
        let named: Vec<&String> = sidebar
            .iter()
            .filter(|line| line.contains("OpenCode"))
            .collect();
        assert_eq!(
            named.len(),
            3,
            "the program and its two untitled runs: {sidebar:#?}"
        );
        assert!(named.iter().any(|line| line.contains("OpenCode #1")));
        assert!(named.iter().any(|line| line.contains("OpenCode #2")));
        let cluster = sidebar
            .iter()
            .position(|line| line.contains("OpenCode"))
            .unwrap();
        let program = sidebar[cluster].find("OpenCode").unwrap();
        let session = sidebar
            .iter()
            .find(|line| line.contains("audit the widget"))
            .unwrap();
        assert!(
            session.find("audit").unwrap() > program,
            "sessions sit deeper than their cluster: {sidebar:#?}"
        );
        // A titled session is named by its task and still says why it is blocked: task and
        // reason answer different questions, so neither hides the other.
        assert!(sidebar.iter().any(|line| line.contains("audit the widget")));
        assert!(sidebar.iter().any(|line| line.contains("fix the flaky")));
        assert!(
            sidebar.iter().any(|line| line.contains("permission")),
            "a titled session shows its reason beside its state: {sidebar:#?}"
        );
        // Until a title exists, the reason is all the row has.
        assert!(sidebar.iter().any(|line| line.contains("answer req")));
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
    });
}

/// A client keeps runs behind the one it shows. Those lean, and carry the background marker at the
/// right of their detail line; the run on screen reads as any other agent does.
#[test]
fn a_run_kept_in_the_background_is_marked() {
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
                let mut publisher =
                    agent_pane(1, agent("codex", "Codex"), None, Some("/home/x/work/rozi"));
                publisher.terminal.published_rows = vec![
                    published_row("shown", "shown task", "working", None, true),
                    published_row("behind", "hidden task", "working", None, false),
                ];
                publisher.agents = ["shown", "behind"]
                    .into_iter()
                    .enumerate()
                    .map(|(index, slot)| {
                        agent_ref(publisher.id, Some(slot), index as u64 + 1).into()
                    })
                    .collect();
                state.current_mut().workspaces[0].panes = vec![publisher];
            }
            backend.render();
            let frame = backend.capture_frame();
            let lines = frame.to_fixed_grid_lines();
            let title = |task: &str| {
                let row = lines.iter().position(|line| line.contains(task)).unwrap();
                let column = lines[row]
                    .chars()
                    .position(|c| c == task.chars().next().unwrap());
                (row, column.unwrap())
            };
            let marker = "⧉";

            let (behind, column) = title("hidden task");
            assert!(
                frame.cell(column as u16, behind as u16).modifiers.italic,
                "a background run's title leans: {lines:#?}"
            );
            assert!(
                lines[behind + 1].contains(marker),
                "its detail line carries the marker: {lines:#?}"
            );
            let status = lines[behind + 1].find("Working").unwrap();
            assert!(
                lines[behind + 1].find(marker).unwrap() > status,
                "the marker sits right of the state word: {lines:#?}"
            );

            let (shown, column) = title("shown task");
            assert!(!frame.cell(column as u16, shown as u16).modifiers.italic);
            assert!(!lines[shown + 1].contains(marker));
        })
        .expect("spawn background run render thread")
        .join()
        .expect("background run render completes");
}

#[test]
fn osc_status_snapshot_rejects_old_generations_and_revisions_and_renders() {
    rozi::test_support::on_large_stack(|| {
        rozi::test_support::isolate_user_dirs();
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        let mut pane = Pane::new(7, 100, FloatRect::default());
        pane.pty_generation = 4;
        pane.title = "build pane".into();
        {
            let state = backend.state_mut();
            agents_fill_the_sidebar(state);
            state.current_mut().workspaces[0].panes = vec![pane];
            state.current_mut().focused_pane = None;
        }
        let epoch = backend.state().runtime_epoch;
        let mut screen = tui_lipan::prelude::TerminalScreen::new(24, 80, 100);
        screen.set_program_status_enabled(true);
        screen.process_bytes(
            b"\x1b]7501;state=working:app=cargo:title=QnVpbGQ=:progress=42:msg=Q29tcGlsaW5n\x07",
        );
        let working = screen.program_status_snapshot();
        let update = |generation, snapshot| Msg::SessionProgramStatusChanged {
            epoch,
            pane_id: 7,
            local: false,
            generation,
            initial: false,
            snapshot,
        };
        backend.dispatch(update(3, working.clone())).unwrap();
        assert!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .program_status
                .records
                .is_empty()
        );
        backend.dispatch(update(4, working.clone())).unwrap();
        backend.render();
        let text = backend.capture_frame().to_fixed_grid_lines().join("\n");
        assert!(text.contains("Build"), "{text}");
        assert!(text.contains("42%"), "{text}");
        screen.process_bytes(b"\x1b]7501;state=error:msg=RmFpbGVk\x07");
        let error = screen.program_status_snapshot();
        backend.dispatch(update(4, error.clone())).unwrap();
        backend.dispatch(update(4, working)).unwrap();
        assert_eq!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .program_status,
            error
        );
        assert!(
            backend.state().current().workspaces[0].panes[0]
                .terminal
                .program_status_attention
        );
        assert!(
            backend.state().current().workspaces[0].panes[0]
                .agent_runtimes()
                .is_empty()
        );
        backend.render();
        let text = backend.capture_frame().to_fixed_grid_lines().join("\n");
        assert!(text.contains('×'), "{text}");
        assert!(text.contains("Failed"), "{text}");
    });
}

/// A pane publishing several sessions folds them under one harness row, which then counts them by
/// state, and opens again on the same arrow.
#[test]
fn a_harness_folds_its_sessions_and_counts_them() {
    rozi::test_support::on_large_stack(|| {
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
                published_row("ses_b", "fix the flaky test", "blocked", None, false),
                published_row("ses_c", "update the changelog", "idle", None, false),
            ];
            publisher.agents = ["ses_a", "ses_b", "ses_c"]
                .into_iter()
                .enumerate()
                .map(|(index, slot)| agent_ref(publisher.id, Some(slot), index as u64 + 1).into())
                .collect();
            state.current_mut().workspaces[0].panes = vec![publisher];
        }
        let sidebar = |backend: &mut TestBackend<AppRoot>| -> Vec<String> {
            backend.render();
            backend
                .capture_frame()
                .to_fixed_grid_lines()
                .iter()
                .map(|line| line.chars().take(32).collect())
                .collect()
        };
        let open = sidebar(&mut backend);
        let harness = open
            .iter()
            .position(|line| line.contains("OpenCode"))
            .unwrap_or_else(|| panic!("harness row: {open:#?}"));
        assert!(open.iter().any(|line| line.contains("fix the flaky")));

        // Folding is the tree's expansion message, keyed by row index like a click on the
        // arrow. The project heading is the first row of the tab body; the harness follows.
        let fold = |backend: &mut TestBackend<AppRoot>, expanded: bool| {
            backend
                .dispatch(rozi::Msg::SidebarRowExpanded {
                    panel: 0,
                    index: 1,
                    expanded,
                })
                .unwrap();
        };
        fold(&mut backend, false);
        let folded = sidebar(&mut backend);
        assert!(
            !folded.iter().any(|line| line.contains("fix the flaky")),
            "folding hides the sessions: {folded:#?}"
        );
        let row = &folded[harness];
        assert!(
            row.contains("▲1"),
            "a folded harness counts what it hides: {folded:#?}"
        );
        assert!(!row.contains('○'), "idle sessions are not counted: {row}");

        fold(&mut backend, true);
        let reopened = sidebar(&mut backend);
        assert!(reopened.iter().any(|line| line.contains("fix the flaky")));
    });
}
