//! What the Projects tab lists: every checkout any session works in, on every host, arranged by
//! repository, with the sessions working there under it.
//!
//! Sessions answer "what is running where"; this answers "what am I working on". A session is not a
//! project: one session can have panes in several repositories, and one repository's checkouts are
//! often spread across several sessions. So the tree is built from where each session's panes and
//! agents actually work, as their session servers resolved it:
//!
//! ```text
//! host → repository → checkout (branch) → session
//! ```
//!
//! The session on screen and connected background attachments are read from their panes. Other
//! sessions come from the project summaries the host monitors collect alongside their agent
//! summaries, so nothing here runs Git or contacts a machine. Each level carries the state of the
//! most urgent agent below it.

use crate::session::protocol::{ProjectRowRef, ProjectSummary};
use crate::session::remote::RemoteTarget;
use crate::state::{
    AgentLocation, RowContent, RowTarget, SidebarTone, State, TreeRow, TreeSection,
};
use crate::view::agents::GlobalAgentRow;

/// One session's work in one checkout, gathered from whichever source reports that session.
#[derive(Clone, Debug)]
struct Work<'a> {
    summary: &'a ProjectSummary,
    host: Option<&'a RemoteTarget>,
}

impl State {
    /// The Projects tab's sections: one per host with work or a failed connection to show.
    pub fn project_sections(&self) -> Vec<TreeSection> {
        let live: Vec<_> = self
            .navigation_attachments()
            .map(|attachment| (attachment, live_project_summaries(attachment)))
            .collect();
        let work = project_work(self, &live);
        let agents = crate::view::agents::global_agent_rows(self);
        let hosts = project_hosts(self, &work);
        let headed = hosts.len() > 1 || hosts.iter().any(Option::is_some);
        hosts
            .into_iter()
            .map(|host| project_section(self, host, &work, &agents, headed))
            .collect()
    }
}

/// Prefer pane-backed session data to the monitors' older copies, including empty attachments.
fn project_work<'a>(
    state: &'a State,
    live: &'a [(&crate::state::Attachment, Vec<ProjectSummary>)],
) -> Vec<Work<'a>> {
    let is_live = |host: Option<&RemoteTarget>, session: &str| {
        live.iter().any(|(attachment, _)| {
            attachment.remote_target.as_ref() == host
                && attachment.session_name.as_deref() == Some(session)
        })
    };
    let mut work: Vec<_> = live
        .iter()
        .flat_map(|(attachment, summaries)| {
            summaries.iter().map(|summary| Work {
                summary,
                host: attachment.remote_target.as_ref(),
            })
        })
        .collect();
    if let Some(snapshot) = &state.local_agent_snapshot {
        work.extend(
            snapshot
                .projects
                .iter()
                .filter(|summary| !is_live(None, &summary.session))
                .map(|summary| Work {
                    summary,
                    host: None,
                }),
        );
    }
    for (target, summaries) in &state.remote.projects {
        work.extend(
            summaries
                .iter()
                .filter(|summary| !is_live(Some(target), &summary.session))
                .map(|summary| Work {
                    summary,
                    host: Some(target),
                }),
        );
    }
    work
}

fn project_hosts<'a>(state: &'a State, work: &[Work<'a>]) -> Vec<Option<&'a RemoteTarget>> {
    let failed = state
        .remote
        .hosts
        .iter()
        .filter(|host| matches!(host.probe, crate::state::HostProbe::Failed(_)))
        .map(|host| Some(&host.target));
    let mut hosts: Vec<_> = work.iter().map(|item| item.host).chain(failed).collect();
    hosts.sort_by_key(|host| host.map(|target| state.remote_target_label(target)));
    hosts.dedup();
    hosts
}

fn project_section(
    state: &State,
    host: Option<&RemoteTarget>,
    work: &[Work<'_>],
    agents: &[GlobalAgentRow],
    headed: bool,
) -> TreeSection {
    let items: Vec<_> = work.iter().filter(|item| item.host == host).collect();
    let mut rows = host_rows(state, host, &items, agents);
    let header = headed.then(|| {
        host.map_or_else(
            || "LOCAL".to_string(),
            |target| state.remote_target_label(target).to_uppercase(),
        )
    });
    let failed = host
        .and_then(|target| state.remote.hosts.get(target))
        .is_some_and(|entry| matches!(entry.probe, crate::state::HostProbe::Failed(_)));
    // A failed monitor has no trustworthy checkout or agent rows. Keep its heading, so the
    // failure is visible and the other host headings do not disappear with it.
    let header = if failed {
        let target = host.expect("a failed host is remote");
        let status =
            crate::view::vocabulary::host_connection_status(state, target, !items.is_empty());
        rows.insert(
            0,
            TreeRow {
                id: "host-status".into(),
                parent: None,
                content: RowContent {
                    label: header.unwrap_or_default(),
                    description: Some(crate::view::vocabulary::host_status_label(status).into()),
                    heading: true,
                    ..RowContent::default()
                },
                target: RowTarget::Inert,
                close: None,
                expandable: false,
                expanded: None,
            },
        );
        None
    } else {
        header
    };
    TreeSection {
        key: host.map_or_else(|| "local".to_string(), RemoteTarget::to_spec),
        header,
        rows,
        lazy: false,
    }
}

/// An attachment's checkouts, from its panes, without waiting for a host poll.
fn live_project_summaries(attachment: &crate::state::Attachment) -> Vec<ProjectSummary> {
    let Some(session) = attachment.session_name.clone() else {
        return Vec::new();
    };
    let mut summaries: Vec<ProjectSummary> = Vec::new();
    let mut panes: Vec<_> = attachment
        .workspaces
        .iter()
        .flat_map(|workspace| &workspace.panes)
        .filter(|pane| pane.id != crate::state::POPUP_PANE_ID && !pane.closing)
        .collect();
    panes.sort_by_key(|pane| pane.id);
    for pane in panes {
        let terminal = &pane.terminal;
        if let Some(root) = terminal.project_root.as_deref() {
            let index = checkout_index(
                &mut summaries,
                &session,
                root,
                terminal.repository.as_ref(),
                terminal.git_branch.as_ref(),
            );
            summaries[index].panes.push(pane.id);
        }
        for row in &terminal.published_rows {
            let Some(project) = row.project.as_ref().filter(|_| row.cwd.is_some()) else {
                continue;
            };
            if terminal.project_root.as_deref() == Some(project.root.as_str()) {
                continue;
            }
            let index = checkout_index(
                &mut summaries,
                &session,
                &project.root,
                project.repository.as_ref(),
                project.branch.as_ref(),
            );
            summaries[index].rows.push(ProjectRowRef {
                pane: pane.id,
                row: row.key(),
            });
        }
    }
    summaries.sort_by(|a, b| a.root.cmp(&b.root));
    summaries
}

/// The summary for the checkout at `root`, added the first time it is seen.
fn checkout_index(
    summaries: &mut Vec<ProjectSummary>,
    session: &str,
    root: &str,
    repository: Option<&String>,
    branch: Option<&String>,
) -> usize {
    if let Some(index) = summaries.iter().position(|summary| summary.root == root) {
        return index;
    }
    summaries.push(ProjectSummary {
        session: session.to_string(),
        root: root.to_string(),
        repository: repository.cloned(),
        branch: branch.cloned(),
        panes: Vec::new(),
        rows: Vec::new(),
    });
    summaries.len() - 1
}

/// The last segment of a path, the name a directory goes by.
fn path_name(path: &str) -> String {
    crate::platform::paths::path_segments(path)
        .last()
        .map_or_else(|| path.to_string(), |name| (*name).to_string())
}

/// `rozi`, or `work/rozi` when another repository on the same host is also called `rozi`.
fn repository_labels(repositories: &[&str]) -> Vec<String> {
    repositories
        .iter()
        .map(|repository| {
            let name = path_name(repository);
            let ambiguous = repositories
                .iter()
                .any(|other| other != repository && path_name(other).eq_ignore_ascii_case(&name));
            if !ambiguous {
                return name;
            }
            let segments = crate::platform::paths::path_segments(repository);
            match segments.split_last() {
                Some((last, rest)) => match rest.last() {
                    Some(parent) => format!("{parent}/{last}"),
                    None => (*last).to_string(),
                },
                None => name,
            }
        })
        .collect()
}

/// Whether an agent row runs in `session` on `host`, in the checkout `summary` describes.
fn agent_in(
    state: &State,
    agent: &GlobalAgentRow,
    host: Option<&RemoteTarget>,
    summary: &ProjectSummary,
    summaries: &[ProjectSummary],
) -> bool {
    let (agent_host, session, pane, row) = match &agent.location {
        AgentLocation::Here { pane, row } => (
            state.current().remote_target.as_ref(),
            agent.session.as_str(),
            *pane,
            row.as_ref(),
        ),
        AgentLocation::OtherSession {
            target,
            session,
            pane,
            row,
        } => (target.as_ref(), session.as_str(), *pane, row.as_ref()),
    };
    agent_host == host
        && session == summary.session
        && crate::session::protocol::checkout_of(summaries, session, pane, row)
            .is_some_and(|found| found.root == summary.root)
}

fn agent_tone(agent: &GlobalAgentRow) -> SidebarTone {
    SidebarTone::from_status(&agent.status, agent.finished_unseen)
}

/// The tone a row reads in when `lead` is the most urgent agent below it: the Agents tab's
/// attention order, so a finished run nobody has read outranks an agent still working at every
/// level of the tree.
fn lead_tone(lead: Option<&GlobalAgentRow>) -> SidebarTone {
    lead.map_or(SidebarTone::Normal, agent_tone)
}

fn most_urgent<'a>(
    agents: impl IntoIterator<Item = Option<&'a GlobalAgentRow>>,
) -> Option<&'a GlobalAgentRow> {
    agents
        .into_iter()
        .flatten()
        .min_by_key(|agent| agent.rank())
}

/// One host's repositories, checkouts, and sessions, as tree rows.
fn host_rows(
    state: &State,
    host: Option<&RemoteTarget>,
    work: &[&Work<'_>],
    agents: &[GlobalAgentRow],
) -> Vec<TreeRow> {
    // Every summary from this host, for placing agents: a row's checkout is decided among all of
    // its session's checkouts, not only the one being drawn.
    let summaries: Vec<ProjectSummary> = work.iter().map(|item| item.summary.clone()).collect();
    let here = |summary: &ProjectSummary| {
        host == state.current().remote_target.as_ref()
            && state.current().session_name.as_deref() == Some(summary.session.as_str())
    };
    let focused = state.focused_pane();
    // A pane may publish rows in other checkouts. Prefer its own checkout, then one published
    // checkout, so the current destination has exactly one marker in this host's tree.
    let focused_checkout = focused.and_then(|pane| {
        summaries
            .iter()
            .filter(|summary| here(summary))
            .find(|summary| summary.panes.contains(&pane))
            .or_else(|| {
                summaries
                    .iter()
                    .filter(|summary| here(summary))
                    .find(|summary| summary.rows.iter().any(|row| row.pane == pane))
            })
            .map(|summary| summary.root.as_str())
    });
    let agents_of = |summary: &ProjectSummary| -> Vec<&GlobalAgentRow> {
        agents
            .iter()
            .filter(|agent| agent_in(state, agent, host, summary, &summaries))
            .collect()
    };

    let mut repositories: Vec<&str> = work
        .iter()
        .map(|item| item.summary.repository_root())
        .collect();
    repositories.sort_unstable();
    repositories.dedup();
    let labels = repository_labels(&repositories);
    let mut order: Vec<usize> = (0..repositories.len()).collect();
    order.sort_by(|a, b| labels[*a].to_lowercase().cmp(&labels[*b].to_lowercase()));

    let mut rows = Vec::new();
    for index in order {
        let repository = repositories[index];
        let repo_id = format!("repo:{repository}");
        let in_repo: Vec<&&Work<'_>> = work
            .iter()
            .filter(|item| item.summary.repository_root() == repository)
            .collect();
        let mut checkouts: Vec<&str> = in_repo
            .iter()
            .map(|item| item.summary.root.as_str())
            .collect();
        checkouts.sort_unstable();
        checkouts.dedup();
        // The primary checkout leads; linked worktrees follow by branch.
        checkouts.sort_by_key(|root| {
            let branch = in_repo
                .iter()
                .find(|item| item.summary.root == *root)
                .and_then(|item| item.summary.branch.clone());
            (
                *root != repository,
                branch.unwrap_or_else(|| path_name(root)),
            )
        });

        let repo_index = rows.len();
        rows.push(TreeRow {
            id: repo_id.clone(),
            parent: None,
            content: RowContent {
                label: labels[index].clone(),
                ..RowContent::default()
            },
            target: RowTarget::Inert,
            close: None,
            expandable: false,
            expanded: None,
        });
        let mut repo_leads = Vec::new();
        for root in checkouts {
            let checkout_id = format!("checkout:{root}");
            let sessions: Vec<&ProjectSummary> = in_repo
                .iter()
                .filter(|item| item.summary.root == root)
                .map(|item| item.summary)
                .collect();
            let branch = sessions.iter().find_map(|summary| summary.branch.clone());
            let checkout_index = rows.len();
            rows.push(TreeRow {
                id: checkout_id.clone(),
                parent: Some(repo_id.clone()),
                content: RowContent {
                    label: branch.unwrap_or_else(|| path_name(root)),
                    ..RowContent::default()
                },
                target: RowTarget::Inert,
                close: None,
                expandable: false,
                expanded: None,
            });
            let mut checkout_leads = Vec::new();
            let mut ordered = sessions;
            ordered.sort_by(|a, b| a.session.cmp(&b.session));
            for summary in ordered {
                let mut session_agents = agents_of(summary);
                session_agents.sort_by_key(|agent| agent.rank());
                let lead = session_agents.first().copied();
                checkout_leads.push(lead);
                let statuses = session_agents
                    .iter()
                    .map(|agent| (agent.status.clone(), agent.finished_unseen))
                    .collect::<Vec<_>>();
                rows[checkout_index]
                    .content
                    .activity_statuses
                    .extend(statuses.iter().cloned());
                rows[repo_index].content.activity_statuses.extend(statuses);
                let active = here(summary) && focused_checkout == Some(summary.root.as_str());
                rows.push(project_session_row(
                    state,
                    host,
                    summary,
                    &session_agents,
                    active,
                ));
            }
            let lead = most_urgent(checkout_leads);
            repo_leads.push(lead);
            rows[checkout_index].content.tone = lead_tone(lead);
        }
        rows[repo_index].content.tone = lead_tone(most_urgent(repo_leads));
    }
    rows
}

fn project_session_row(
    state: &State,
    host: Option<&RemoteTarget>,
    summary: &ProjectSummary,
    agents: &[&GlobalAgentRow],
    active: bool,
) -> TreeRow {
    let is_here = host == state.current().remote_target.as_ref()
        && state.current().session_name.as_deref() == Some(summary.session.as_str());
    // Keep the current destination when already in this checkout. Otherwise land on
    // the most urgent agent, then a pane or published row working here.
    let landing = state
        .focused_pane()
        .filter(|_| active)
        .map(|pane| (pane, None))
        .or_else(|| {
            agents
                .first()
                .map(|agent| (agent.location.pane(), agent.location.row().cloned()))
        })
        .or_else(|| summary.panes.first().map(|pane| (*pane, None)))
        .or_else(|| {
            summary
                .rows
                .first()
                .map(|row| (row.pane, Some(row.row.clone())))
        });
    let target = match landing {
        Some((pane, row)) if is_here => RowTarget::Agent(AgentLocation::Here { pane, row }),
        Some((pane, row)) => RowTarget::Agent(AgentLocation::OtherSession {
            target: host.cloned(),
            session: summary.session.clone(),
            pane,
            row,
        }),
        None => RowTarget::Inert,
    };
    let label = crate::view::vocabulary::session_name(&summary.session).to_string();
    TreeRow {
        id: format!("session:{}:{}", summary.root, summary.session),
        parent: Some(format!("checkout:{}", summary.root)),
        content: RowContent {
            label,
            description: project_description(state, host, summary, agents),
            activity_statuses: agents
                .iter()
                .map(|agent| (agent.status.clone(), agent.finished_unseen))
                .collect(),
            tone: lead_tone(agents.first().copied()),
            active,
            ..RowContent::default()
        },
        target,
        close: None,
        expandable: false,
        expanded: None,
    }
}

/// Unheld sessions are necessarily running in this view. Keep the work summary's room while
/// retaining states that distinguish the client's current, background, and lost connections.
fn project_description(
    state: &State,
    host: Option<&RemoteTarget>,
    summary: &ProjectSummary,
    agents: &[&GlobalAgentRow],
) -> Option<String> {
    use crate::view::vocabulary::{SessionPresence, fields, session_presence_for};
    let presence = session_presence_for(state, &summary.session, host);
    let status = (!matches!(
        presence,
        SessionPresence::Running | SessionPresence::Current
    ))
    .then(|| presence.label().to_string());
    let facts = session_description(agents, summary);
    let description = fields(status.into_iter().chain(facts));
    (!description.is_empty()).then_some(description)
}

/// Count panes when no agent runs here; agent activity is carried by the compact status counts.
fn session_description(agents: &[&GlobalAgentRow], summary: &ProjectSummary) -> Option<String> {
    (agents.is_empty() && !summary.panes.is_empty())
        .then(|| crate::view::vocabulary::count(summary.panes.len(), "pane"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::protocol::AgentSummary;

    fn summary(
        session: &str,
        root: &str,
        repository: Option<&str>,
        branch: &str,
        panes: &[crate::state::PaneId],
    ) -> ProjectSummary {
        ProjectSummary {
            session: session.into(),
            root: root.into(),
            repository: repository.map(str::to_string),
            branch: Some(branch.into()),
            panes: panes.to_vec(),
            rows: Vec::new(),
        }
    }

    fn agent(session: &str, pane: crate::state::PaneId, state: &str) -> AgentSummary {
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

    fn state() -> State {
        State::new(
            crate::config::Config::default(),
            tui_lipan::Theme::default(),
        )
    }

    fn outline(sections: &[TreeSection]) -> Vec<String> {
        let tab = crate::config::SidebarTabId::new("projects");
        crate::state::flatten(&tab, sections, &crate::state::TreeExpansion::new())
            .into_iter()
            .map(|item| match item {
                crate::state::TreeItem::Spacer => "-".into(),
                crate::state::TreeItem::Header(header) => format!("#{header}"),
                crate::state::TreeItem::Row(line) => format!(
                    "{}{} {:?}{}",
                    "  ".repeat(line.branch.depth()),
                    line.row.content.label,
                    line.row.content.tone,
                    line.row
                        .content
                        .description
                        .map(|text| format!(" [{text}]"))
                        .unwrap_or_default(),
                ),
                crate::state::TreeItem::Loading(_) => "…".into(),
            })
            .collect()
    }

    /// Two sessions in two checkouts of one repository, one of them with a blocked agent: the
    /// repository and the checkout both roll up to blocked, and the primary checkout leads.
    #[test]
    fn checkouts_group_under_their_repository_with_agent_state_rolled_up() {
        let mut state = state();
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            sessions: Vec::new(),
            agents: vec![agent("fix", 3, "blocked"), agent("dev", 1, "working")],
            projects: vec![
                summary(
                    "fix",
                    "/x/rozi/.worktrees/fix",
                    Some("/x/rozi"),
                    "fix/ssh",
                    &[3],
                ),
                summary("dev", "/x/rozi", None, "master", &[1, 2]),
                summary("dev", "/x/tui-lipan", None, "main", &[4]),
            ],
        });
        let sections = state.project_sections();
        let statuses = |id: &str| {
            sections[0]
                .rows
                .iter()
                .find(|row| row.id == id)
                .unwrap()
                .content
                .activity_statuses
                .iter()
                .map(|(status, _)| status.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(statuses("repo:/x/rozi"), ["working", "blocked"]);
        assert_eq!(statuses("checkout:/x/rozi"), ["working"]);
        assert_eq!(statuses("checkout:/x/rozi/.worktrees/fix"), ["blocked"]);
        assert_eq!(statuses("session:/x/rozi:dev"), ["working"]);
        assert!(statuses("repo:/x/tui-lipan").is_empty());
        assert_eq!(
            outline(&state.project_sections()),
            [
                "rozi Blocked",
                "  master Working",
                "    dev Working",
                "  fix/ssh Blocked",
                "    fix Blocked",
                "tui-lipan Normal",
                "  main Normal",
                "    dev Normal [1 pane]",
            ]
        );
    }

    /// The tree rolls up by what to look at next: a finished run nobody has read outranks an agent
    /// still working, at the checkout and at the repository.
    #[test]
    fn an_unread_finish_outranks_work_still_in_progress() {
        let mut state = state();
        let mut finished = agent("fix", 3, "idle");
        finished.completions = 1;
        state.agent_attention.observe(
            &finished.reference,
            0,
            crate::state::FirstSighting::Baseline,
        );
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            sessions: Vec::new(),
            agents: vec![finished, agent("dev", 1, "working")],
            projects: vec![
                summary(
                    "fix",
                    "/x/rozi/.worktrees/fix",
                    Some("/x/rozi"),
                    "fix/ssh",
                    &[3],
                ),
                summary("dev", "/x/rozi", None, "master", &[1]),
            ],
        });
        assert_eq!(
            outline(&state.project_sections()),
            [
                "rozi Success",
                "  master Working",
                "    dev Working",
                "  fix/ssh Success",
                "    fix Success",
            ]
        );
    }

    /// Work on another machine is headed by that machine, and this one is then headed too.
    #[test]
    fn hosts_are_headed_once_a_remote_host_has_work() {
        let mut state = state();
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            sessions: Vec::new(),
            agents: Vec::new(),
            projects: vec![summary("dev", "/x/rozi", None, "master", &[1])],
        });
        state.remote.projects.insert(
            RemoteTarget::Alias("workbox".into()),
            vec![summary("backend", "/srv/api", None, "main", &[1])],
        );
        let lines = outline(&state.project_sections());
        assert_eq!(lines[0], "#LOCAL");
        assert!(lines.contains(&"#WORKBOX".to_string()), "{lines:?}");
    }

    #[test]
    fn failed_hosts_keep_their_heading_until_recovered_disconnected_or_forgotten() {
        use crate::state::HostProbe;
        let mut state = state();
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            projects: vec![summary("dev", "/x/rozi", None, "master", &[1])],
            agents: Vec::new(),
            sessions: Vec::new(),
        });
        let target = RemoteTarget::Alias("workbox".into());
        state.remote.hosts.seed(
            &state.config.remote,
            std::slice::from_ref(&target),
            &[],
            &[],
        );
        state.remote.hosts.get_mut(&target).unwrap().probe =
            HostProbe::Failed("SSH timed out".into());
        let sections = state.project_sections();
        assert_eq!(sections[0].header.as_deref(), Some("LOCAL"));
        let remote = &sections[1];
        assert_eq!(remote.key, target.to_spec());
        assert_eq!(remote.header, None);
        assert_eq!(
            remote.rows.len(),
            1,
            "untrusted checkout and agent rows stay absent"
        );
        assert!(remote.rows[0].content.heading);
        assert_eq!(remote.rows[0].content.label, "WORKBOX");
        assert_eq!(
            remote.rows[0].content.description.as_deref(),
            Some("Unreachable")
        );
        assert_eq!(remote.rows[0].target, RowTarget::Inert);

        state.remote.hosts.get_mut(&target).unwrap().probe = HostProbe::Reached;
        state.remote.projects.insert(
            target.clone(),
            vec![summary("api", "/srv/api", None, "main", &[2])],
        );
        let recovered = state.project_sections();
        assert_eq!(recovered[1].header.as_deref(), Some("WORKBOX"));
        assert_eq!(recovered[1].rows[0].content.label, "api");
        assert!(!recovered[1].rows[0].content.heading);

        state.remote.projects.remove(&target);
        state.remote.hosts.get_mut(&target).unwrap().probe = HostProbe::Idle;
        assert_eq!(state.project_sections().len(), 1);
        assert_eq!(state.project_sections()[0].header, None);

        state.remote.hosts.get_mut(&target).unwrap().probe =
            HostProbe::Failed("SSH timed out".into());
        state.remote.hosts.seed(&state.config.remote, &[], &[], &[]);
        assert_eq!(state.project_sections().len(), 1);
    }

    #[test]
    fn failed_host_is_visible_without_any_local_work() {
        let mut state = state();
        let target = RemoteTarget::Alias("workbox".into());
        state.remote.hosts.seed(
            &state.config.remote,
            std::slice::from_ref(&target),
            &[],
            &[],
        );
        state.remote.hosts.get_mut(&target).unwrap().probe =
            crate::state::HostProbe::Failed("SSH timed out".into());
        let items = crate::state::flatten(
            &crate::config::SidebarTabId::new("projects"),
            &state.project_sections(),
            &crate::state::TreeExpansion::new(),
        );
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].line().unwrap().row.content.description.as_deref(),
            Some("Unreachable")
        );
    }

    #[test]
    fn project_descriptions_keep_held_session_states_and_hide_redundant_running() {
        use crate::state::{Attachment, ConnectionState};
        let mut state = state();
        let summary = summary("dev", "/x/rozi", None, "master", &[1, 2]);
        assert_eq!(
            project_description(&state, None, &summary, &[]).as_deref(),
            Some("2 panes")
        );
        state.current_mut().session_name = Some("dev".into());
        assert_eq!(
            project_description(&state, None, &summary, &[]).as_deref(),
            Some("2 panes")
        );
        state.current_mut().session_name = Some("other".into());
        for (connection, expected) in [
            (ConnectionState::Connected, "Background · 2 panes"),
            (ConnectionState::Reconnecting, "Reconnecting · 2 panes"),
            (ConnectionState::Disconnected, "Offline · 2 panes"),
        ] {
            let mut attachment = Attachment::new();
            attachment.session_name = Some("dev".into());
            attachment.connection = connection;
            state.background.insert(1, attachment);
            assert_eq!(
                project_description(&state, None, &summary, &[]).as_deref(),
                Some(expected)
            );
        }
    }

    /// Two different repositories with the same directory name are told apart by their parent.
    #[test]
    fn same_named_repositories_are_disambiguated() {
        assert_eq!(
            repository_labels(&["/a/work/api", "/a/oss/api", "/a/rozi"]),
            ["work/api", "oss/api", "rozi"]
        );
    }

    #[test]
    fn only_the_focused_checkouts_session_row_has_an_active_marker() {
        let mut state = state();
        state.current_mut().session_name = Some("dev".into());
        let summaries = [
            summary("dev", "/x/rozi", None, "master", &[1]),
            summary("dev", "/x/tui-lipan", None, "main", &[2]),
            summary("other", "/x/rozi", None, "master", &[1]),
        ];
        let work = summaries
            .iter()
            .map(|summary| Work {
                summary,
                host: None,
            })
            .collect::<Vec<_>>();
        for (focused, expected) in [
            (Some(1), vec!["session:/x/rozi:dev"]),
            (Some(2), vec!["session:/x/tui-lipan:dev"]),
            (None, vec![]),
        ] {
            state.current_mut().focused_pane = focused;
            let active = host_rows(&state, None, &work.iter().collect::<Vec<_>>(), &[])
                .into_iter()
                .filter(|row| row.content.active)
                .map(|row| row.id)
                .collect::<Vec<_>>();
            assert_eq!(active, expected);
        }
    }
}
