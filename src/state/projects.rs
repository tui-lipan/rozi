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
//! The session on screen is read from its live panes. Every other session — local or on a connected
//! host — comes from the project summaries the host monitors already collect alongside their agent
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
    /// The Projects tab's sections: one per host with work to show.
    pub fn project_sections(&self) -> Vec<TreeSection> {
        let live = live_project_summaries(self);
        let here_target = self.current().remote_target.as_ref();
        let here_session = self.current().session_name.as_deref();
        let mut work: Vec<Work<'_>> = Vec::new();
        for summary in &live {
            work.push(Work {
                summary,
                host: here_target,
            });
        }
        // Another source's copy of the session on screen is staler than its live panes.
        let is_here = |host: Option<&RemoteTarget>, session: &str| {
            host == here_target && here_session == Some(session)
        };
        if let Some(snapshot) = &self.local_agent_snapshot {
            for summary in &snapshot.projects {
                if !is_here(None, &summary.session) {
                    work.push(Work {
                        summary,
                        host: None,
                    });
                }
            }
        }
        for (target, summaries) in &self.remote.projects {
            for summary in summaries {
                if !is_here(Some(target), &summary.session) {
                    work.push(Work {
                        summary,
                        host: Some(target),
                    });
                }
            }
        }

        let agents = crate::view::agents::global_agent_rows(self);
        let mut hosts: Vec<Option<&RemoteTarget>> = Vec::new();
        for item in &work {
            if !hosts.contains(&item.host) {
                hosts.push(item.host);
            }
        }
        hosts.sort_by_key(|host| host.map(|target| self.remote_target_label(target)));
        // A header says which machine a section is; with only this one, it says nothing.
        let headed = hosts.len() > 1 || hosts.iter().any(Option::is_some);
        hosts
            .into_iter()
            .map(|host| {
                let items: Vec<_> = work.iter().filter(|item| item.host == host).collect();
                TreeSection {
                    key: host.map_or_else(|| "local".to_string(), RemoteTarget::to_spec),
                    header: headed.then(|| match host {
                        Some(target) => self.remote_target_label(target).to_uppercase(),
                        None => "LOCAL".to_string(),
                    }),
                    rows: host_rows(self, host, &items, &agents),
                    lazy: false,
                }
            })
            .collect()
    }
}

/// The session on screen's checkouts, from its live panes: what its server would report, without
/// waiting for a poll.
fn live_project_summaries(state: &State) -> Vec<ProjectSummary> {
    let Some(session) = state.current().session_name.clone() else {
        return Vec::new();
    };
    let mut summaries: Vec<ProjectSummary> = Vec::new();
    let mut panes: Vec<_> = state
        .current()
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
                let tone = lead_tone(lead);
                checkout_leads.push(lead);
                let is_here = here(summary);
                let active = is_here && focused_checkout == Some(summary.root.as_str());
                // Keep the current destination when already in this checkout. Otherwise land on
                // the most urgent agent, then a pane or published row working here.
                let landing = focused
                    .filter(|_| active)
                    .map(|pane| (pane, None))
                    .or_else(|| {
                        session_agents
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
                    Some((pane, row)) if is_here => {
                        RowTarget::Agent(AgentLocation::Here { pane, row })
                    }
                    Some((pane, row)) => RowTarget::Agent(AgentLocation::OtherSession {
                        target: host.cloned(),
                        session: summary.session.clone(),
                        pane,
                        row,
                    }),
                    None => RowTarget::Inert,
                };
                let label = crate::view::vocabulary::session_name(&summary.session).to_string();
                rows.push(TreeRow {
                    id: format!("session:{root}:{}", summary.session),
                    parent: Some(checkout_id.clone()),
                    content: RowContent {
                        label,
                        description: Some(crate::view::vocabulary::fields([
                            crate::view::vocabulary::session_presence_for(
                                state,
                                &summary.session,
                                host,
                            )
                            .label()
                            .to_string(),
                            session_description(&session_agents, summary).unwrap_or_default(),
                        ])),
                        tone,
                        active,
                        ..RowContent::default()
                    },
                    target,
                    close: None,
                    expandable: false,
                    expanded: None,
                });
            }
            let lead = most_urgent(checkout_leads);
            repo_leads.push(lead);
            rows[checkout_index].content.tone = lead_tone(lead);
        }
        rows[repo_index].content.tone = lead_tone(most_urgent(repo_leads));
    }
    rows
}

/// What a session row says about the work it holds: the one agent by name, a count of several,
/// or how many panes have no agent at all.
fn session_description(agents: &[&GlobalAgentRow], summary: &ProjectSummary) -> Option<String> {
    match agents {
        [] => {
            let panes = summary.panes.len();
            (panes > 0).then(|| crate::view::vocabulary::count(panes, "pane"))
        }
        [agent] => Some(agent.agent.clone()),
        several => Some(crate::view::vocabulary::count(several.len(), "agent")),
    }
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
        assert_eq!(
            outline(&state.project_sections()),
            [
                "rozi Blocked",
                "  master Working",
                "    dev Working [Running · Codex]",
                "  fix/ssh Blocked",
                "    fix Blocked [Running · Codex]",
                "tui-lipan Normal",
                "  main Normal",
                "    dev Normal [Running · 1 pane]",
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
                "    dev Working [Running · Codex]",
                "  fix/ssh Success",
                "    fix Success [Running · Codex]",
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
