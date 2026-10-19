//! What the global Agents view lists, as a pure projection of `State`.
//!
//! Kept apart from the overlay that draws it because two sources feed it and the rule joining them
//! is the interesting part: the session in front of the user publishes live pane state, while other
//! local sessions and connected hosts contribute semantic summaries, placed in their projects by
//! the same monitors' project summaries. The overlay renders whatever comes out; this is where the
//! boundary between the two is decided.

use crate::session::protocol::pane_status;
use crate::state::{AgentLocation, State};

/// One row of the global Agents view: an agent, where it is running, and what it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct GlobalAgentRow {
    pub location: AgentLocation,
    /// The agent's own name (`Codex`, `Claude Code`). Several runs of one program share it.
    pub agent: String,
    /// What the agent works on, when it named its work (`Fix SSH reconnect`): what tells one run of
    /// a program from another. Never a terminal title, which renames the row with every tool call.
    pub task: Option<String>,
    /// The machine it is on, `None` for this one.
    pub host: Option<String>,
    pub session: String,
    /// The status exactly as reported. Rendered through the Agents tab's own vocabulary, so the two
    /// surfaces never spell the same state two ways.
    pub status: String,
    /// Why the agent is in its state, as it said (`Permission required`).
    pub reason: Option<String>,
    /// The agent finished a run this client has not looked at. See
    /// [`crate::state::AgentAttention`].
    pub finished_unseen: bool,
    /// The time the row shows, by the Agents tab's rule: how long a working or blocked state has
    /// held, what a finished run cost, and nothing for an idle agent. See
    /// [`crate::view::sidebar::agents::elapsed`].
    ///
    /// Live panes only. Local and remote summaries expose a change time, but the UI has no
    /// maintained elapsed timer for them; remote clocks may also differ from this client's.
    pub elapsed: Option<std::time::Duration>,
    /// The repository the agent works in, named after its primary checkout when the agent works in
    /// a linked worktree.
    pub project: Option<String>,
    /// The branch checked out where the agent works.
    pub branch: Option<String>,
    /// Which of several rows this is that would otherwise read exactly the same - one program's
    /// untitled runs in one checkout - and `None` for every row that reads apart on its own.
    pub ordinal: Option<usize>,
}

impl GlobalAgentRow {
    /// The host and session identity a tab uses, including the attached session's target.
    pub fn session_tab(&self, state: &State) -> crate::state::AgentPickerTab {
        let target = match &self.location {
            AgentLocation::Here { .. } => state.current().remote_target.clone(),
            AgentLocation::OtherSession { target, .. } => target.clone(),
        };
        crate::state::AgentPickerTab::Session {
            target,
            session: self.session.clone(),
        }
    }

    pub fn in_tab(&self, state: &State, tab: &crate::state::AgentPickerTab) -> bool {
        *tab == crate::state::AgentPickerTab::All || self.session_tab(state) == *tab
    }

    /// How much this row wants the user, lowest first. See
    /// [`crate::session::protocol::ActivityState::attention_rank`].
    pub fn rank(&self) -> u8 {
        crate::session::protocol::ActivityState::from_status(&self.status)
            .attention_rank(self.finished_unseen)
    }

    /// Whether this agent wants the user: waiting on them, failed, or finished a run nobody has
    /// looked at. The sidebar's Agents tab lists only these; the Agents view lists every agent.
    pub fn needs_attention(&self) -> bool {
        self.rank() <= crate::session::protocol::ActivityState::Done.attention_rank(true)
    }

    pub fn is_here(&self) -> bool {
        matches!(self.location, AgentLocation::Here { .. })
    }

    /// What the row is called: its task when it named one, else the agent, numbered only when
    /// another row would otherwise read the same.
    pub fn title(&self) -> String {
        let title = self.task.as_deref().unwrap_or(&self.agent);
        match self.ordinal {
            Some(number) => format!("{title} #{number}"),
            None => title.to_string(),
        }
    }

    /// `rozi (fix-login)`: the project and the branch checked out there.
    pub fn place(&self) -> Option<String> {
        let project = self.project.as_deref()?;
        Some(match self.branch.as_deref() {
            Some(branch) => format!("{project} ({branch})"),
            None => project.to_string(),
        })
    }

    /// The state word the row shows, `Done` for a finished run nobody has looked at.
    pub fn status_label(&self) -> String {
        crate::view::sidebar::agents::row_status_label(&self.status, self.finished_unseen)
    }

    /// `Fix SSH reconnect · Codex · backend@workbox` — what, who, then where, on one searchable
    /// line. The place belongs in the label rather than the description because it is the half a
    /// user types.
    pub fn label(&self) -> String {
        crate::view::vocabulary::fields(
            std::iter::once(self.title())
                .chain(self.task.is_some().then(|| self.agent.clone()))
                .chain([crate::view::vocabulary::session_at(
                    &self.session,
                    self.host.as_deref(),
                )]),
        )
    }

    /// What else a search should find a row by: its host's spec, the project and branch it works
    /// in, and why it is in its state.
    pub fn search_aliases(&self, state: &State) -> Vec<String> {
        let target = match &self.location {
            AgentLocation::Here { .. } => state.current().remote_target.as_ref(),
            AgentLocation::OtherSession { target, .. } => target.as_ref(),
        };
        target
            .map(|target| target.to_spec())
            .into_iter()
            .chain(self.place())
            .chain(self.reason.clone())
            .collect()
    }

    /// `Blocked · Permission required · 4m00s · rozi (fix-login)`: the state, why, for how long,
    /// and in which checkout.
    pub fn description(&self) -> String {
        let mut parts = vec![self.status_label()];
        parts.extend(self.reason.clone());
        parts.extend(self.elapsed.map(crate::view::sidebar::agents::format_age));
        parts.extend(self.place());
        crate::view::vocabulary::fields(parts)
    }
}

/// The repository an Activity row works in, named after its primary checkout when it is a linked
/// worktree, and the branch it has checked out.
fn row_project(row: &crate::view::sidebar::agents::AgentRow) -> (Option<String>, Option<String>) {
    let path = row
        .repository
        .as_deref()
        .or(row.project_root.as_deref())
        .or(row.cwd.as_deref());
    (path.map(project_name), row.branch.clone())
}

fn project_name(path: &str) -> String {
    crate::platform::paths::path_segments(path)
        .last()
        .map_or_else(|| path.to_string(), |name| (*name).to_string())
}

/// A summary's row: the agent another session reports, placed in the checkout its session's project
/// summaries put it in.
fn summary_row(
    state: &State,
    agent: &crate::session::protocol::AgentSummary,
    target: Option<&crate::session::remote::RemoteTarget>,
    projects: &[crate::session::protocol::ProjectSummary],
) -> GlobalAgentRow {
    let checkout =
        crate::session::protocol::checkout_of(projects, &agent.session, agent.pane(), agent.row());
    GlobalAgentRow {
        location: AgentLocation::OtherSession {
            target: target.cloned(),
            session: agent.session.clone(),
            pane: agent.pane(),
            row: agent.row().cloned(),
        },
        agent: agent.label.clone(),
        task: agent
            .task
            .as_deref()
            .and_then(|task| crate::view::sidebar::agents::task_title(task, &agent.label)),
        host: target.map(|target| state.remote_target_label(target)),
        session: agent.session.clone(),
        status: agent.state.as_str().to_string(),
        reason: agent.reason.clone(),
        finished_unseen: state.agent_attention.summary_finished_unseen(agent),
        elapsed: None,
        project: checkout.map(|checkout| project_name(checkout.repository_root())),
        branch: checkout.and_then(|checkout| checkout.branch.clone()),
        ordinal: None,
    }
}

/// Where a row sits, for numbering rows that would otherwise read the same: its pane, then its
/// place among that pane's agents. Stable while states change, and the order Activity numbers the
/// same runs in, so one run carries one number everywhere.
type Seat = (crate::state::PaneId, usize);

/// Number the rows that would otherwise read exactly the same - same title, same agent beside a
/// task, same session and host, same project and branch - by where they sit. Every other row stays
/// unnumbered.
fn number_twins(rows: &mut [(GlobalAgentRow, Seat)]) {
    // What a row shows: its title, the agent beside a task, and where it runs. Two programs with
    // the same task already read apart by the agent's name, as they do in Activity.
    let key = |row: &GlobalAgentRow| {
        (
            row.title(),
            row.task.is_some().then(|| row.agent.clone()),
            row.session.clone(),
            row.host.clone(),
            row.place(),
        )
    };
    let keys: Vec<_> = rows.iter().map(|(row, _)| key(row)).collect();
    let ordinals: Vec<Option<usize>> = (0..rows.len())
        .map(|index| {
            let mut twins: Vec<usize> = (0..rows.len())
                .filter(|other| keys[*other] == keys[index])
                .collect();
            if twins.len() < 2 {
                return None;
            }
            twins.sort_by_key(|other| rows[*other].1);
            twins
                .iter()
                .position(|other| *other == index)
                .map(|at| at + 1)
        })
        .collect();
    for ((row, _), ordinal) in rows.iter_mut().zip(ordinals) {
        row.ordinal = ordinal;
    }
}

/// Every agent this client knows about, the ones most in need of attention first.
///
/// Ordering is by state and then by identity, never by timestamp. The only clock the remote rows
/// carry belongs to the machine that stamped it, so sorting one host's minutes against another's
/// would reshuffle the list whenever two machines disagreed about the time. A stable order is also
/// what lets a monitor poll land while the overlay is open without moving the cursor onto a
/// different agent than the one under it a moment ago.
pub(crate) fn global_agent_rows(state: &State) -> Vec<GlobalAgentRow> {
    let here_target = state.current().remote_target.clone();
    let here_host = here_target
        .as_ref()
        .map(|target| state.remote_target_label(target));
    let here_session = state.current().session_name.clone();
    let mut rows: Vec<(GlobalAgentRow, Seat)> = Vec::new();
    if let Some(session) = here_session.as_deref() {
        // The Activity tab's rows, minus its activities: only the agent projection is listed.
        for row in crate::view::sidebar::agents::agent_rows(state) {
            let Some(identity) = row.identity.clone() else {
                continue;
            };
            let (project, branch) = row_project(&row);
            let elapsed = crate::view::sidebar::agents::elapsed(
                row.status.as_deref().unwrap_or(pane_status::IDLE),
                row.finished_unseen,
                row.age,
                row.run,
            )
            .map(|(elapsed, _)| elapsed);
            let seat = (row.pane_id, row.order);
            let global = GlobalAgentRow {
                location: AgentLocation::Here {
                    pane: row.pane_id,
                    row: row.slot.as_ref().map(|slot| slot.key.clone()),
                },
                task: row.task.as_deref().and_then(|task| {
                    crate::view::sidebar::agents::task_title(task, &identity.label)
                }),
                agent: identity.label,
                host: here_host.clone(),
                session: session.to_string(),
                status: row.status.unwrap_or_default(),
                reason: row.reason,
                finished_unseen: row.finished_unseen,
                elapsed,
                project,
                branch,
                ordinal: None,
            };
            rows.push((global, seat));
        }
    }
    if let Some(snapshot) = &state.local_agent_snapshot {
        // Summaries arrive in pane order and, within a pane, in the order it publishes them.
        for (index, agent) in snapshot.agents.iter().enumerate() {
            if here_target.is_none() && here_session.as_deref() == Some(&agent.session) {
                continue;
            }
            let row = summary_row(state, agent, None, &snapshot.projects);
            rows.push((row, (agent.pane(), index)));
        }
    }
    for (target, agents) in &state.remote.agents {
        let projects = state
            .remote
            .projects
            .get(target)
            .map_or(&[][..], Vec::as_slice);
        for (index, agent) in agents.iter().enumerate() {
            // The session on screen is already listed from its live panes, which are fresher than a
            // snapshot taken between polls. Two answers for one agent, disagreeing by up to a poll
            // interval and sitting next to each other, is worse than either alone.
            if here_target.as_ref() == Some(target)
                && here_session.as_deref() == Some(&agent.session)
            {
                continue;
            }
            let row = summary_row(state, agent, Some(target), projects);
            rows.push((row, (agent.pane(), index)));
        }
    }
    number_twins(&mut rows);
    let mut rows: Vec<GlobalAgentRow> = rows.into_iter().map(|(row, _)| row).collect();
    rows.sort_by(|a, b| {
        a.rank()
            .cmp(&b.rank())
            .then_with(|| b.is_here().cmp(&a.is_here()))
            .then_with(|| a.host.cmp(&b.host))
            .then_with(|| a.session.cmp(&b.session))
            .then_with(|| a.location.pane().cmp(&b.location.pane()))
            .then_with(|| a.location.row().cmp(&b.location.row()))
    });
    rows
}

/// The row the Agents view opens on: whichever one most wants attention.
pub(crate) fn first_agent_location(state: &State) -> Option<AgentLocation> {
    global_agent_rows(state)
        .into_iter()
        .next()
        .map(|row| row.location)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::AgentSummary;
    use crate::session::remote::RemoteTarget;

    fn summary(session: &str, pane: crate::state::PaneId, state: &str) -> AgentSummary {
        AgentSummary {
            reference: crate::session::protocol::AgentRef::for_test(pane, 0, None),
            session: session.into(),
            agent: "codex".into(),
            label: "Codex".into(),
            state: crate::session::protocol::AgentState::from_status(state),
            reason: None,
            task: None,
            completions: 0,
            changed_at: 0,
        }
    }

    fn state_with_hosts(hosts: &[(&str, Vec<AgentSummary>)]) -> State {
        let mut state = State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        );
        for (alias, agents) in hosts {
            state
                .remote
                .agents
                .insert(RemoteTarget::Alias((*alias).into()), agents.clone());
        }
        state
    }

    /// The whole point of the view: the thing that wants an answer is the thing under the cursor
    /// when it opens, whichever machine it happens to be on.
    #[test]
    fn rows_lead_with_what_wants_attention_across_every_host() {
        let state = state_with_hosts(&[
            ("alpha", vec![summary("dev", 1, "working")]),
            (
                "beta",
                vec![summary("api", 2, "idle"), summary("api", 3, "blocked")],
            ),
        ]);
        let rows = global_agent_rows(&state);
        let order: Vec<_> = rows
            .iter()
            .map(|row| (row.host.as_deref().unwrap(), row.status.as_str()))
            .collect();
        assert_eq!(
            order,
            vec![("beta", "blocked"), ("alpha", "working"), ("beta", "idle")]
        );
        assert_eq!(
            first_agent_location(&state),
            Some(AgentLocation::OtherSession {
                target: Some(RemoteTarget::Alias("beta".into())),
                session: "api".into(),
                pane: 3,
                row: None,
            })
        );
    }

    /// A row has to say which machine it is on before it says anything else about itself — the
    /// label is what a user searches, and "Codex" alone is the one thing they already know.
    #[test]
    fn a_remote_row_names_its_host_and_session() {
        let state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "blocked")])]);
        let rows = global_agent_rows(&state);
        assert_eq!(rows[0].label(), "Codex · backend@workbox");
        assert_eq!(rows[0].description(), "Blocked");
    }

    /// Several sessions of one program differ only by what each works on and where: the task
    /// leads the searchable label, and the reason, project, and branch fill the description.
    #[test]
    fn a_published_session_names_its_work_and_its_worktree() {
        let row = GlobalAgentRow {
            location: AgentLocation::Here {
                pane: 1,
                row: Some(crate::session::protocol::RowKey::new("sessions", "fix")),
            },
            agent: "Claude Code".into(),
            task: Some("fix login redirect".into()),
            host: None,
            session: "dev".into(),
            status: "blocked".into(),
            reason: Some("Permission required".into()),
            finished_unseen: false,
            elapsed: Some(std::time::Duration::from_secs(240)),
            project: Some("rozi".into()),
            branch: Some("worktree-fix".into()),
            ordinal: None,
        };
        assert_eq!(row.title(), "fix login redirect");
        assert_eq!(row.label(), "fix login redirect · Claude Code · dev");
        assert_eq!(
            row.description(),
            "Blocked · Permission required · 4m00s · rozi (worktree-fix)"
        );

        let agent_row = crate::view::sidebar::agents::AgentRow {
            pane_id: 1,
            workspace_index: 0,
            pane_index: 0,
            title: "Claude Code".into(),
            status: None,
            activity: None,
            age: None,
            run: None,
            cwd: Some("/x/rozi/.claude/worktrees/fix".into()),
            cwd_host: None,
            project_root: Some("/x/rozi/.claude/worktrees/fix".into()),
            subpath: None,
            branch: Some("worktree-fix".into()),
            repository: Some("/x/rozi".into()),
            finished_unseen: false,
            slot: None,
            identity: Some(crate::session::protocol::AgentIdentity::new(
                "claude",
                "Claude Code",
            )),
            task: None,
            reason: None,
            order: 0,
            screen_title: None,
        };
        assert_eq!(
            row_project(&agent_row),
            (Some("rozi".into()), Some("worktree-fix".into()))
        );
    }

    /// The list is what to look at next: an agent waiting on the user, then a finished run nobody
    /// has read, and only then one still working, which needs nothing from anyone yet.
    #[test]
    fn unread_finishes_outrank_agents_still_working() {
        let mut finished = summary("api", 3, "idle");
        finished.completions = 1;
        let mut state = state_with_hosts(&[(
            "workbox",
            vec![
                summary("api", 1, "working"),
                summary("api", 2, "idle"),
                finished.clone(),
                summary("api", 4, "blocked"),
                summary("api", 5, "done"),
            ],
        )]);
        state.agent_attention.observe(
            &finished.reference,
            0,
            crate::state::FirstSighting::Baseline,
        );
        for agent in &state.remote.agents[&RemoteTarget::Alias("workbox".into())] {
            state.agent_attention.observe(
                &agent.reference,
                agent.completions,
                crate::state::FirstSighting::Baseline,
            );
        }
        let order: Vec<_> = global_agent_rows(&state)
            .iter()
            .map(|row| (row.location.pane(), row.status_label()))
            .collect();
        assert_eq!(
            order,
            vec![
                (4, "Blocked".to_string()),
                (3, "Done".to_string()),
                (1, "Working".to_string()),
                (5, "Done".to_string()),
                (2, "Idle".to_string()),
            ]
        );
    }

    /// An agent in another session is placed in its project by the same monitor's project
    /// summaries, so the view can say where it works without attaching.
    #[test]
    fn a_summary_is_placed_in_the_checkout_its_session_reports() {
        let mut agent = summary("api", 4, "working");
        agent.task = Some("✳ fix reconnect".into());
        agent.reason = Some("Running tests".into());
        let mut state = state_with_hosts(&[("workbox", vec![agent])]);
        state.remote.projects.insert(
            RemoteTarget::Alias("workbox".into()),
            vec![crate::session::protocol::ProjectSummary {
                session: "api".into(),
                root: "/src/rozi/.worktrees/fix".into(),
                repository: Some("/src/rozi".into()),
                branch: Some("fix-ssh".into()),
                panes: vec![4],
                rows: Vec::new(),
            }],
        );
        let rows = global_agent_rows(&state);
        assert_eq!(rows[0].task.as_deref(), Some("fix reconnect"));
        assert_eq!(rows[0].place().as_deref(), Some("rozi (fix-ssh)"));
        assert!(
            rows[0]
                .search_aliases(&state)
                .contains(&"rozi (fix-ssh)".to_string())
        );
    }

    /// Two untitled runs of one program in one session and checkout read the same, so each is
    /// numbered by where it sits - in the label a search matches too. A row that reads apart on its
    /// own keeps no number.
    #[test]
    fn rows_that_would_read_the_same_are_numbered() {
        let untitled = |pane| {
            let mut agent = summary("api", pane, "idle");
            agent.label = "OpenCode".into();
            agent
        };
        let mut titled = untitled(3);
        titled.task = Some("fix reconnect".into());
        let state = state_with_hosts(&[("workbox", vec![untitled(5), untitled(2), titled])]);
        let mut titles: Vec<_> = global_agent_rows(&state)
            .iter()
            .map(|row| (row.location.pane(), row.title(), row.label()))
            .collect();
        titles.sort();
        assert_eq!(
            titles,
            vec![
                (
                    2,
                    "OpenCode #1".to_string(),
                    "OpenCode #1 · api@workbox".to_string()
                ),
                (
                    3,
                    "fix reconnect".to_string(),
                    "fix reconnect · OpenCode · api@workbox".to_string()
                ),
                (
                    5,
                    "OpenCode #2".to_string(),
                    "OpenCode #2 · api@workbox".to_string()
                ),
            ]
        );
    }

    /// Two programs working on the same task read apart by their names, so neither is numbered.
    #[test]
    fn the_same_task_in_two_programs_is_not_numbered() {
        let working = |pane, label: &str| {
            let mut agent = summary("api", pane, "working");
            agent.label = label.into();
            agent.task = Some("Review PR #183".into());
            agent
        };
        let state = state_with_hosts(&[(
            "workbox",
            vec![working(1, "Claude Code"), working(2, "Codex")],
        )]);
        let rows = global_agent_rows(&state);
        assert!(rows.iter().all(|row| row.ordinal.is_none()));
        assert!(rows.iter().all(|row| row.title() == "Review PR #183"));
    }

    /// An elapsed time is the field a reader trusts without checking, and the only timestamp a
    /// summary carries was stamped by another machine's clock. See [`GlobalAgentRow::elapsed`].
    #[test]
    fn a_remote_row_claims_no_elapsed_time() {
        let state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "working")])]);
        assert_eq!(global_agent_rows(&state)[0].elapsed, None);
    }

    /// The session on screen reports itself through its live panes. A monitor snapshot of the same
    /// session is the staler of the two answers, and listing both would show them side by side.
    #[test]
    fn the_session_on_screen_is_not_listed_twice() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state_with_hosts(&[("workbox", vec![summary("backend", 1, "blocked")])]);
        assert_eq!(global_agent_rows(&state).len(), 1);

        state.current_mut().session_name = Some("backend".into());
        state.current_mut().remote_target = Some(target);
        // No panes in this bare state, so the live half contributes nothing and the snapshot half
        // is suppressed: the row is gone rather than duplicated.
        assert!(global_agent_rows(&state).is_empty());
    }

    /// A different session on the same host is a different agent, and stays listed.
    #[test]
    fn another_session_on_the_attached_host_stays_listed() {
        let target = RemoteTarget::Alias("workbox".into());
        let mut state = state_with_hosts(&[(
            "workbox",
            vec![summary("backend", 1, "blocked"), summary("web", 2, "idle")],
        )]);
        state.current_mut().session_name = Some("backend".into());
        state.current_mut().remote_target = Some(target);
        let rows = global_agent_rows(&state);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session, "web");
    }

    #[test]
    fn other_local_sessions_join_remote_rows_without_repeating_the_current_session() {
        let mut state = state_with_hosts(&[("workbox", vec![summary("dev", 9, "working")])]);
        state.current_mut().session_name = Some("dev".into());
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            projects: Vec::new(),
            sessions: Vec::new(),
            agents: vec![summary("dev", 1, "idle"), summary("api", 2, "blocked")],
        });
        let rows = global_agent_rows(&state);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label(), "Codex · api");
        assert_eq!(
            rows[0].location,
            AgentLocation::OtherSession {
                target: None,
                session: "api".into(),
                pane: 2,
                row: None,
            }
        );
        assert_eq!(rows[1].label(), "Codex · dev@workbox");
        assert_eq!(rows[0].elapsed, None);
    }
}
