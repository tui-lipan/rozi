//! The global Agents view: open it, move its cursor, and land on what it points at.
//!
//! Everything the view lists is derived state: panes in the foreground and connected parked
//! sessions, and host-monitor snapshots for other sessions. There is nothing to build or refresh.
//! This module owns the one act the view exists for: turning "Codex is blocked on backend@workbox" into
//! being *there*, across an attach when the session is somewhere else.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::{AgentLocation, AgentPickerState, PendingAgentJump};

pub(crate) fn open_agent_picker(ctx: &mut Context<AppRoot>) -> Update {
    // Land on the first row, which is the most urgent one: the view is ordered by what wants
    // attention, so the answer to "what needs me right now" is already under the cursor.
    let selected = crate::view::agents::first_agent_location(&ctx.state);
    ctx.state.agent_picker = Some(AgentPickerState::new(selected));
    ctx.state.commands_dirty = true;
    crate::ops::focus::request_agent_picker_focus(ctx);
    Update::full()
}

/// Sessions with agents, plus the active page even if its last agent disappears.
pub(crate) fn picker_tabs(state: &crate::state::State) -> Vec<crate::state::AgentPickerTab> {
    use crate::state::AgentPickerTab;
    let mut tabs = Vec::new();
    for row in crate::view::agents::global_agent_rows(state) {
        let tab = row.session_tab(state);
        if !tabs.contains(&tab) {
            tabs.push(tab);
        }
    }
    if let Some(picker) = &state.agent_picker
        && picker.tab != AgentPickerTab::All
        && !tabs.contains(&picker.tab)
    {
        tabs.push(picker.tab.clone());
    }
    tabs.sort_by_key(|tab| {
        (
            matches!(
                tab,
                AgentPickerTab::Session {
                    target: Some(_),
                    ..
                }
            ),
            tab.label(state),
        )
    });
    tabs.push(AgentPickerTab::All);
    tabs
}

pub(crate) fn select_tab(ctx: &mut Context<AppRoot>, index: usize) -> Update {
    let Some(tab) = picker_tabs(&ctx.state).get(index).cloned() else {
        return Update::none();
    };
    let Some(picker) = ctx.state.agent_picker.as_mut() else {
        return Update::none();
    };
    picker.tab = tab;
    picker.selected = None;
    sync_picker_selection(&mut ctx.state);
    crate::ops::focus::request_agent_picker_focus(ctx);
    Update::full()
}

/// Keep a cursor attached to a matching row in the active session as snapshots arrive.
pub(crate) fn sync_picker_selection(state: &mut crate::state::State) -> bool {
    let Some(picker) = &state.agent_picker else {
        return false;
    };
    let items = crate::view::agents::global_agent_rows(state)
        .iter()
        .filter(|row| row.in_tab(state, &picker.tab))
        .map(|row| {
            SearchItem::new(row.label(), row.location.clone())
                .aliases(row.search_aliases(state))
                .description(ItemDescription::new().right(row.description()))
        })
        .collect::<Vec<_>>();
    let matches = tui_lipan::rank_search_palette_indices_with_mode(
        &items,
        picker.input.text(),
        SearchMatchMode::Hybrid,
        |_, _, score| score as f64,
    );
    let selected = picker
        .selected
        .as_ref()
        .filter(|selected| {
            matches
                .iter()
                .any(|index| &items[*index].value == *selected)
        })
        .cloned()
        .or_else(|| matches.first().map(|index| items[*index].value.clone()));
    if selected == picker.selected {
        return false;
    }
    state
        .agent_picker
        .as_mut()
        .expect("picker checked above")
        .selected = selected;
    true
}

pub(crate) fn close_agent_picker(ctx: &mut Context<AppRoot>) -> Update {
    if ctx.state.agent_picker.take().is_none() {
        return Update::none();
    }
    ctx.state.commands_dirty = true;
    if ctx.state.is_launcher() {
        return Update::full();
    }
    crate::ops::focus::request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn query_changed(ctx: &mut Context<AppRoot>, query: String) -> Update {
    if let Some(picker) = ctx.state.agent_picker.as_mut() {
        picker.input.set_text(query);
    }
    Update::full()
}

pub(crate) fn select(ctx: &mut Context<AppRoot>, location: AgentLocation) -> Update {
    if let Some(picker) = ctx.state.agent_picker.as_mut() {
        picker.selected = Some(location);
    }
    Update::full()
}

/// Go to the agent this row describes, wherever it is running.
///
/// A pane in the session on screen is a focus change and nothing more. Anywhere else, the session
/// has to arrive first, so the destination is recorded and [`land_on_pending_agent`] finishes the
/// gesture — immediately when the session was merely parked in the background, and after the
/// attach lands otherwise.
pub(crate) fn activate(ctx: &mut Context<AppRoot>, location: AgentLocation) -> Update {
    match location.clone() {
        AgentLocation::Here { pane, row } => {
            // Landed on before the overlay closes, so the focus the close returns to the "current"
            // pane is the pane the user just asked for rather than the one they came from.
            land_on_pane(ctx, pane, row.as_ref());
            close_agent_picker(ctx)
        }
        AgentLocation::OtherSession { .. } => jump_to_other_session(ctx, location, |ctx, _| {
            // Installing a session dismisses the pickers that led into it, so an overlay still
            // standing here means the activation refused (an incompatible server, say) and left
            // its reason on screen.
            ctx.state.agent_picker.is_some()
        }),
    }
}

/// Go to the agent a sidebar row describes. The same destination as [`activate`], from a surface
/// that stays on screen: there is no overlay to close, and none whose survival could say the
/// attach was refused.
pub(crate) fn activate_from_sidebar(ctx: &mut Context<AppRoot>, location: AgentLocation) -> Update {
    match location {
        AgentLocation::Here { pane, row } => {
            if land_on_pane(ctx, pane, row.as_ref()) {
                Update::full()
            } else {
                Update::none()
            }
        }
        AgentLocation::OtherSession { .. } => {
            jump_to_other_session(ctx, location, |_, entry| {
                // The one refusal `activate_discovered_session` makes before attaching: discovery
                // already found the server unable to speak to this client.
                matches!(
                    entry.status,
                    crate::session::discovery::DiscoveredSessionStatus::Unknown
                )
            })
        }
    }
}

/// Every agent this client knows about, in the Agents tab's order, for `agents-overview`.
pub(crate) fn overview(state: &crate::state::State) -> Vec<crate::control::GlobalAgentInfo> {
    crate::view::agents::global_agent_rows(state)
        .into_iter()
        .map(|row| {
            let here = row.is_here();
            let state_word = if row.finished_unseen
                && crate::session::protocol::ActivityState::from_status(&row.status)
                    .is_successful_terminal()
            {
                crate::session::protocol::pane_status::DONE.to_string()
            } else {
                row.status.clone()
            };
            let (target, pane, slot) = match row.location {
                AgentLocation::Here { pane, row: slot } => {
                    (state.current().remote_target.clone(), pane, slot)
                }
                AgentLocation::OtherSession {
                    target,
                    pane,
                    row: slot,
                    ..
                } => (target, pane, slot),
            };
            crate::control::GlobalAgentInfo {
                location: crate::control::AgentLocationSpec {
                    host: target.map(|target| target.to_spec()),
                    session: row.session,
                    pane,
                    row: slot,
                },
                label: row.agent,
                task: row.task,
                state: state_word,
                reason: row.reason,
                project: row.project,
                branch: row.branch,
                here,
            }
        })
        .collect()
}

/// The agent location a caller described, if it names a session this client knows.
pub(crate) fn location_from_spec(
    state: &crate::state::State,
    spec: &crate::control::AgentLocationSpec,
) -> std::result::Result<AgentLocation, String> {
    let target = spec
        .host
        .as_deref()
        .map(crate::session::remote::parse_remote_target)
        .transpose()?;
    let here = state.current().remote_target == target
        && state.current().session_name.as_deref() == Some(spec.session.as_str());
    if here {
        return Ok(AgentLocation::Here {
            pane: spec.pane,
            row: spec.row.clone(),
        });
    }
    if connected_parked_session(state, target.as_ref(), &spec.session).is_none()
        && session_for_agent(state, target.as_ref(), &spec.session).is_none()
    {
        return Err(match &spec.host {
            Some(host) => format!("no session `{}` on a connected host `{host}`", spec.session),
            None => format!("no running local session `{}`", spec.session),
        });
    }
    Ok(AgentLocation::OtherSession {
        target,
        session: spec.session.clone(),
        pane: spec.pane,
        row: spec.row.clone(),
    })
}

/// Open the session an agent lives in, then land on its pane once that session is on screen.
///
/// `refused` reports, after activation, whether the session is not coming. A refused destination is
/// dropped rather than left to fire on some unrelated later attach to the same name.
fn jump_to_other_session(
    ctx: &mut Context<AppRoot>,
    location: AgentLocation,
    refused: impl FnOnce(&Context<AppRoot>, &crate::session::discovery::DiscoveredSession) -> bool,
) -> Update {
    let AgentLocation::OtherSession {
        target,
        session,
        pane,
        row,
    } = location
    else {
        return Update::none();
    };
    let jump = PendingAgentJump {
        target,
        session,
        pane,
        row,
    };
    // A held connection is an independent source of both the row and its destination. A host
    // monitor can fail without affecting it, so switching must not depend on discovery.
    if let Some(parked) = connected_parked_session(&ctx.state, jump.target.as_ref(), &jump.session)
    {
        ctx.state.pending_agent_jump = Some(jump);
        let update = crate::ops::session::switch_to_parked(ctx, parked);
        return finish_agent_jump(ctx, update);
    }
    let Some(entry) = session_for_agent(&ctx.state, jump.target.as_ref(), &jump.session) else {
        // An unheld session must still be reported by discovery before a new attach is attempted.
        return Update::none();
    };
    ctx.state.pending_agent_jump = Some(jump);
    let update = crate::ops::session::activate_discovered_session(ctx, entry.clone());
    if refused(ctx, &entry) {
        ctx.state.pending_agent_jump = None;
        return update;
    }
    finish_agent_jump(ctx, update)
}

fn connected_parked_session(
    state: &crate::state::State,
    target: Option<&crate::session::remote::RemoteTarget>,
    session: &str,
) -> Option<crate::state::AttachmentId> {
    state.parked_attachment_id(session, target).filter(|id| {
        state.background.get(id).is_some_and(|attachment| {
            attachment.connection == crate::state::ConnectionState::Connected
        })
    })
}

fn finish_agent_jump(ctx: &mut Context<AppRoot>, update: Update) -> Update {
    // Switching a parked session is synchronous; a fresh attach leaves the jump pending.
    if land_on_pending_agent(ctx) {
        Update::full()
    } else {
        update
    }
}

fn session_for_agent(
    state: &crate::state::State,
    target: Option<&crate::session::remote::RemoteTarget>,
    session: &str,
) -> Option<crate::session::discovery::DiscoveredSession> {
    let sessions = match target {
        Some(_) => state.remote.live_sessions.as_slice(),
        None => state
            .local_agent_snapshot
            .as_ref()
            .map(|snapshot| snapshot.sessions.as_slice())
            .unwrap_or_default(),
    };
    sessions
        .iter()
        .find(|entry| entry.name == session && entry.remote_target.as_ref() == target)
        .cloned()
}

/// Take the recorded destination if the session that owns it is now in the foreground, and land on
/// it. Reports whether it did, so callers can tell a finished jump from one still in flight.
///
/// Matched by session identity rather than by attach epoch, because switching to a parked
/// attachment installs a session without an attach ever happening and both routes have to resolve
/// the same pending jump on the same rule.
pub(crate) fn land_on_pending_agent(ctx: &mut Context<AppRoot>) -> bool {
    if !pending_agent_has_arrived(&ctx.state) {
        return false;
    }
    let Some(jump) = ctx.state.pending_agent_jump.take() else {
        return false;
    };
    land_on_pane(ctx, jump.pane, jump.row.as_ref())
}

/// Whether the session the recorded destination named is the one now in the foreground.
fn pending_agent_has_arrived(state: &crate::state::State) -> bool {
    state.pending_agent_jump.as_ref().is_some_and(|jump| {
        state.current().session_name.as_deref() == Some(jump.session.as_str())
            && state.current().remote_target == jump.target
    })
}

/// Focus the pane and, for an agent that is one of several published in it, ask the program to
/// bring that row on screen.
///
/// `false` when the pane is not in this client's layout — what a summary taken by a poll before the
/// pane exited looks like from here. The session is still opened either way, which is the half of
/// the request that survives its destination going away.
fn land_on_pane(
    ctx: &mut Context<AppRoot>,
    pane: crate::state::PaneId,
    row: Option<&crate::session::protocol::RowKey>,
) -> bool {
    let Some(row) = row else {
        return crate::ops::focus::focus_pane_anywhere(ctx, pane);
    };
    if crate::pane::lifecycle::find_pane_mut(&mut ctx.state, pane).is_none() {
        return false;
    }
    crate::update::sidebar::activation::activate_published_row(ctx, pane, row.clone());
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Msg;
    use crate::layout::tiling::append_tiled_window;
    use crate::session::remote::RemoteTarget;
    use crate::state::Pane;
    use tui_lipan::TestBackend;

    fn backend_with_panes(ids: &[crate::state::PaneId]) -> TestBackend<AppRoot> {
        let mut backend = TestBackend::new(AppRoot::default());
        backend.set_viewport(Rect {
            x: 0,
            y: 0,
            w: 100,
            h: 30,
        });
        let state = backend.state_mut();
        let rect = FloatRect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 24.0,
        };
        for id in ids {
            state.current_mut().workspaces[0]
                .panes
                .push(Pane::new(*id, 100, rect));
            append_tiled_window(&mut state.current_mut().workspaces[0], *id);
        }
        backend
    }

    /// An agent in the session already on screen is a focus change and nothing else — no attach, no
    /// round trip, and the overlay is gone by the time the pane has the keyboard.
    /// The overview names where every agent runs in a form `agent-go` takes back, and a location
    /// naming a session this client does not know is refused rather than guessed at.
    #[test]
    fn overview_locations_resolve_back_to_where_the_agent_runs() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        state.remote.agents.insert(
            RemoteTarget::Alias("workbox".into()),
            vec![crate::session::protocol::AgentSummary {
                reference: crate::session::protocol::AgentRef::for_test(4, 0, None),
                session: "backend".into(),
                agent: "codex".into(),
                label: "Codex".into(),
                state: crate::session::protocol::AgentState::from_status("blocked"),
                reason: None,
                task: None,
                completions: 0,
                changed_at: 0,
            }],
        );
        state
            .remote
            .live_sessions
            .push(crate::session::discovery::DiscoveredSession {
                name: "backend".into(),
                status: crate::session::discovery::DiscoveredSessionStatus::Running {
                    panes: 1,
                    clients: 0,
                    has_layout: true,
                },
                origin: Default::default(),
                ephemeral: false,
                host: Some("workbox".into()),
                remote_target: Some(RemoteTarget::Alias("workbox".into())),
            });
        let overview = overview(&state);
        assert_eq!(overview.len(), 1);
        assert_eq!(overview[0].location.host.as_deref(), Some("workbox"));
        assert_eq!(overview[0].state, "blocked");
        assert!(!overview[0].here);
        assert_eq!(
            location_from_spec(&state, &overview[0].location),
            Ok(AgentLocation::OtherSession {
                target: Some(RemoteTarget::Alias("workbox".into())),
                session: "backend".into(),
                pane: 4,
                row: None,
            })
        );
        let mut unknown = overview[0].location.clone();
        unknown.session = "gone".into();
        assert!(location_from_spec(&state, &unknown).is_err());
    }

    #[test]
    fn a_row_in_this_session_goes_straight_to_its_pane() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[2, 3]);
            backend
                .dispatch(Msg::AgentPickerActivate(AgentLocation::Here {
                    pane: 3,
                    row: None,
                }))
                .expect("activate");
            assert_eq!(backend.state().current().focused_pane, Some(3));
            assert!(backend.state().agent_picker.is_none());
        });
    }

    /// Session rows and agent summaries arrive in the same monitor poll, so a summary with no
    /// session beside it describes something the host has stopped reporting. Nothing is opened, and
    /// nothing is left queued to fire on a later attach that happens to share the name.
    #[test]
    fn a_row_whose_session_is_gone_opens_nothing() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend
                .dispatch(Msg::AgentPickerActivate(AgentLocation::OtherSession {
                    target: Some(RemoteTarget::Alias("workbox".into())),
                    session: "backend".into(),
                    pane: 7,
                    row: None,
                }))
                .expect("activate");
            assert!(backend.state().pending_agent_jump.is_none());
        });
    }

    #[test]
    fn parked_rows_activate_without_any_discovery_entry() {
        for remote in [false, true] {
            for entry_point in ["projects", "agents", "picker", "spec"] {
                crate::test_support::on_large_stack(move || {
                    use crate::session::protocol::{
                        AgentIdentity, DetectedAgent, DetectedAgentState,
                    };
                    use crate::state::{Attachment, ConnectionState, RowTarget};
                    let mut backend = backend_with_panes(&[2, 3]);
                    let target = remote.then(|| RemoteTarget::Alias("workbox".into()));
                    {
                        let state = backend.state_mut();
                        state.command_link = None;
                        let current = state.current_mut();
                        current.session_name = Some("backend".into());
                        current.remote_target = target.clone();
                        current.connection = ConnectionState::Connected;
                        current.session_attached = true;
                        current.pending_session_attach = None;
                        current.focused_pane = Some(2);
                        for pane in &mut current.workspaces[0].panes {
                            pane.terminal.cwd = Some("/srv/api".into());
                            pane.terminal.project_root = Some("/srv/api".into());
                            pane.terminal.git_branch = Some("main".into());
                        }
                        current.workspaces[0]
                            .panes
                            .iter_mut()
                            .find(|pane| pane.id == 3)
                            .unwrap()
                            .terminal
                            .detected_agent = Some(DetectedAgent {
                            agent: AgentIdentity::new("codex", "Codex").into(),
                            state: DetectedAgentState::Blocked,
                        });
                        let mut foreground = Attachment::new();
                        foreground.session_name = Some("foreground".into());
                        state.park_current(42, foreground);
                        state.remote.live_sessions.clear();
                        state.local_agent_snapshot = None;
                        state.remote.agents.clear();
                        state.remote.projects.clear();
                    }
                    let rows = overview(backend.state());
                    assert_eq!(
                        rows.len(),
                        1,
                        "the pane-backed parked agent remains visible"
                    );
                    let spec = &rows[0].location;
                    let expected = AgentLocation::OtherSession {
                        target: target.clone(),
                        session: "backend".into(),
                        pane: 3,
                        row: None,
                    };
                    assert_eq!(
                        location_from_spec(backend.state(), spec),
                        Ok(expected.clone())
                    );
                    let projects = backend.state().project_sections();
                    assert!(
                        projects
                            .iter()
                            .flat_map(|section| &section.rows)
                            .any(|row| row.target == RowTarget::Agent(expected.clone()))
                    );
                    // A retained but disconnected picture is not an independently live destination.
                    backend
                        .state_mut()
                        .background
                        .get_mut(&42)
                        .unwrap()
                        .connection = ConnectionState::Disconnected;
                    assert!(location_from_spec(backend.state(), spec).is_err());
                    backend
                        .state_mut()
                        .background
                        .get_mut(&42)
                        .unwrap()
                        .connection = ConnectionState::Connected;

                    match entry_point {
                        "projects" | "agents" => {
                            let tab = if entry_point == "projects" {
                                crate::config::SidebarTab::Projects
                            } else {
                                crate::config::SidebarTab::Agents
                            };
                            let state = backend.state_mut();
                            state.config.sidebar.tabs = vec![tab.clone()];
                            state.sidebar.panels[0].tabs = vec![tab.id()];
                            state.sidebar.panels[0].active_tab = Some(tab.id());
                            let index = state
                                .sidebar_rows(&tab)
                                .iter()
                                .position(|row| row.target == RowTarget::Agent(expected.clone()))
                                .unwrap();
                            backend
                                .update_level(Msg::SidebarRowActivate { panel: 0, index })
                                .unwrap();
                        }
                        "picker" => {
                            backend.state_mut().agent_picker =
                                Some(AgentPickerState::new(Some(expected.clone())));
                            backend
                                .update_level(Msg::AgentPickerActivate(expected))
                                .unwrap();
                        }
                        "spec" => {
                            let location = location_from_spec(backend.state(), spec).unwrap();
                            backend
                                .update_level(Msg::AgentPickerActivate(location))
                                .unwrap();
                        }
                        _ => unreachable!(),
                    }
                    let state = backend.state();
                    assert_eq!(state.current().session_name.as_deref(), Some("backend"));
                    assert_eq!(state.current().remote_target, target);
                    assert_eq!(state.runtime_epoch, 42);
                    assert_eq!(state.current().focused_pane, Some(3));
                    assert!(
                        state.current().pending_session_attach.is_none(),
                        "switch without a new attach"
                    );
                    assert!(state.pending_agent_jump.is_none());
                    assert!(state.agent_picker.is_none());
                    assert!(!state.background.contains_key(&42));
                });
            }
        }
    }

    #[test]
    fn agent_destination_uses_the_matching_local_or_remote_session() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        let target = RemoteTarget::Alias("workbox".into());
        let row = |remote_target| crate::session::discovery::DiscoveredSession {
            name: "dev".into(),
            status: crate::session::discovery::DiscoveredSessionStatus::Running {
                panes: 1,
                clients: 0,
                has_layout: true,
            },
            origin: Default::default(),
            ephemeral: false,
            host: None,
            remote_target,
        };
        state.local_agent_snapshot = Some(crate::session::discovery::LocalAgentSnapshot {
            projects: Vec::new(),
            sessions: vec![row(None)],
            agents: Vec::new(),
        });
        state.remote.live_sessions.push(row(Some(target.clone())));
        assert_eq!(
            session_for_agent(&state, None, "dev")
                .unwrap()
                .remote_target,
            None
        );
        assert_eq!(
            session_for_agent(&state, Some(&target), "dev")
                .unwrap()
                .remote_target,
            Some(target)
        );
        assert!(session_for_agent(&state, None, "missing").is_none());
    }

    /// The view's reason to exist, end to end: an agent on a machine this client is not looking at
    /// is on screen, named by host and session, with the state that makes it worth going to.
    #[test]
    fn the_overlay_lists_an_agent_on_another_machine() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            backend.state_mut().remote.agents.insert(
                RemoteTarget::Alias("workbox".into()),
                vec![crate::session::protocol::AgentSummary {
                    reference: crate::session::protocol::AgentRef::for_test(4, 0, None),
                    session: "backend".into(),
                    agent: "codex".into(),
                    label: "Codex".into(),
                    state: crate::session::protocol::AgentState::from_status("blocked"),
                    reason: None,
                    task: None,
                    completions: 0,
                    changed_at: 0,
                }],
            );
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
                .expect("open the agents view");
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame
                    .lines()
                    .any(|line| line.contains("Codex") && line.contains("backend@workbox")),
                "the row names who and where:\n{frame}"
            );
            assert!(
                frame.contains("Blocked"),
                "and the highlighted row's detail says what it is waiting on:\n{frame}"
            );
        });
    }

    #[test]
    fn the_overlay_lists_an_agent_in_another_local_session() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            backend.state_mut().local_agent_snapshot =
                Some(crate::session::discovery::LocalAgentSnapshot {
                    projects: Vec::new(),
                    sessions: Vec::new(),
                    agents: vec![crate::session::protocol::AgentSummary {
                        reference: crate::session::protocol::AgentRef::for_test(4, 0, None),
                        session: "backend".into(),
                        agent: "codex".into(),
                        label: "Codex".into(),
                        state: crate::session::protocol::AgentState::from_status("blocked"),
                        reason: None,
                        task: None,
                        completions: 0,
                        changed_at: 0,
                    }],
                });
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
                .expect("open the agents view");
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame
                    .lines()
                    .any(|line| line.contains("Codex") && line.contains("backend")),
                "{frame}"
            );
            assert!(frame.contains("Blocked"), "{frame}");
        });
    }

    fn tabbed_backend() -> TestBackend<AppRoot> {
        let mut backend = backend_with_panes(&[]);
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        let summary = |pane, label: &str, status: &str| crate::session::protocol::AgentSummary {
            reference: crate::session::protocol::AgentRef::for_test(pane, 0, None),
            session: "dev".into(),
            agent: "codex".into(),
            label: label.into(),
            state: crate::session::protocol::AgentState::from_status(status),
            reason: None,
            task: None,
            completions: 0,
            changed_at: 0,
        };
        backend.state_mut().local_agent_snapshot =
            Some(crate::session::discovery::LocalAgentSnapshot {
                projects: Vec::new(),
                sessions: Vec::new(),
                agents: vec![summary(1, "Local Codex", "idle")],
            });
        backend.state_mut().remote.agents.insert(
            RemoteTarget::Alias("workbox".into()),
            vec![
                summary(2, "Remote Codex", "blocked"),
                summary(3, "Remote Claude", "working"),
            ],
        );
        backend
            .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
            .unwrap();
        backend.render();
        backend
    }

    #[test]
    fn keyboard_tabs_scope_by_session_keep_focus_and_omit_repeated_location() {
        crate::test_support::on_large_stack(|| {
            use crate::state::AgentPickerTab;
            let mut backend = tabbed_backend();
            assert_eq!(
                picker_tabs(backend.state())
                    .iter()
                    .map(|tab| tab.label(backend.state()))
                    .collect::<Vec<_>>(),
                ["dev", "dev@workbox", "All"]
            );
            assert!(matches!(
                backend.state().agent_picker.as_ref().unwrap().selected,
                Some(AgentLocation::OtherSession { pane: 2, .. })
            ));
            for (index, name) in [(0, "Local Codex"), (1, "Remote Codex")] {
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Tab,
                        mods: KeyMods::NONE,
                    })
                    .unwrap();
                backend.render();
                assert_eq!(
                    backend.state().agent_picker.as_ref().unwrap().tab,
                    picker_tabs(backend.state())[index]
                );
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char('x'),
                        mods: KeyMods::NONE,
                    })
                    .unwrap();
                assert_eq!(
                    backend.state().agent_picker.as_ref().unwrap().input.text(),
                    "x"
                );
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Backspace,
                        mods: KeyMods::NONE,
                    })
                    .unwrap();
                backend.render();
                let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
                let row = frame.lines().find(|line| line.contains(name)).unwrap();
                assert!(!row.contains("dev"), "session belongs in the tab: {row}");
                if index == 0 {
                    assert!(!frame.contains("Remote Codex"), "{frame}");
                } else {
                    assert!(!frame.contains("Local Codex"), "{frame}");
                }
            }
            backend
                .send_key(KeyEvent {
                    code: KeyCode::Right,
                    mods: KeyMods::NONE,
                })
                .unwrap();
            assert_eq!(
                backend.state().agent_picker.as_ref().unwrap().tab,
                AgentPickerTab::All
            );
            backend.render();
            backend
                .send_key(KeyEvent {
                    code: KeyCode::BackTab,
                    mods: KeyMods {
                        shift: true,
                        ..KeyMods::NONE
                    },
                })
                .unwrap();
            assert_eq!(
                backend
                    .state()
                    .agent_picker
                    .as_ref()
                    .unwrap()
                    .tab
                    .label(backend.state()),
                "dev@workbox"
            );
        });
    }

    #[test]
    fn colliding_remote_targets_have_distinct_agent_tabs_rows_and_search() {
        crate::test_support::on_large_stack(|| {
            let mut backend = tabbed_backend();
            backend.set_viewport(Rect {
                x: 0,
                y: 0,
                w: 180,
                h: 30,
            });
            let alias = RemoteTarget::Alias("workbox".into());
            let url = RemoteTarget::Url {
                user: None,
                host: "workbox".into(),
                port: None,
            };
            let mut agent = backend.state().remote.agents[&alias][0].clone();
            agent.label = "URL Codex".into();
            backend
                .state_mut()
                .remote
                .agents
                .insert(url.clone(), vec![agent]);
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            let tabs = picker_tabs(backend.state());
            let labels: Vec<_> = tabs.iter().map(|tab| tab.label(backend.state())).collect();
            assert!(
                labels.contains(&"dev@workbox (workbox)".into()),
                "{labels:?}"
            );
            assert!(
                labels.contains(&"dev@workbox (ssh://workbox)".into()),
                "{labels:?}"
            );
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame
                    .lines()
                    .any(|line| line.contains("Remote Codex")
                        && line.contains("dev@workbox (workbox)")),
                "{frame}"
            );
            assert!(
                frame.lines().any(|line| line.contains("URL Codex")
                    && line.contains("dev@workbox (ssh://workbox)")),
                "{frame}"
            );
            for ch in "ssh://workbox".chars() {
                backend
                    .send_key(KeyEvent {
                        code: KeyCode::Char(ch),
                        mods: KeyMods::NONE,
                    })
                    .unwrap();
            }
            backend.render();
            let picker = backend.state().agent_picker.as_ref().unwrap();
            assert!(
                matches!(&picker.selected, Some(AgentLocation::OtherSession { target: Some(target), .. }) if target == &url)
            );
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame.contains("URL Codex") && !frame.contains("Remote Codex"),
                "{frame}"
            );
            let index = tabs.iter().position(|tab| matches!(tab, crate::state::AgentPickerTab::Session { target: Some(target), .. } if target == &url)).unwrap();
            backend.dispatch(Msg::AgentPickerTab(index)).unwrap();
            backend.render();
            assert!(
                matches!(&backend.state().agent_picker.as_ref().unwrap().selected, Some(AgentLocation::OtherSession { target: Some(target), .. }) if target == &url)
            );
        });
    }

    /// All lists every row, so a highlight carried over from another tab would land mid-list.
    #[test]
    fn switching_tabs_lands_on_the_top_row() {
        crate::test_support::on_large_stack(|| {
            let mut backend = tabbed_backend();
            let top_of_all = backend
                .state()
                .agent_picker
                .as_ref()
                .unwrap()
                .selected
                .clone();
            let last_session = picker_tabs(backend.state()).len() - 2;
            backend.dispatch(Msg::AgentPickerTab(last_session)).unwrap();
            let on_session = backend
                .state()
                .agent_picker
                .as_ref()
                .unwrap()
                .selected
                .clone();
            assert!(on_session.is_some());

            for tab in 0..=last_session + 1 {
                backend.dispatch(Msg::AgentPickerTab(tab)).unwrap();
                let state = backend.state();
                let picker = state.agent_picker.as_ref().unwrap();
                let top = crate::view::agents::global_agent_rows(state)
                    .into_iter()
                    .find(|row| row.in_tab(state, &picker.tab))
                    .map(|row| row.location);
                assert_eq!(picker.selected, top, "tab {tab}");
            }
            assert_eq!(
                backend.state().agent_picker.as_ref().unwrap().selected,
                top_of_all,
                "All starts at its top row again"
            );
        });
    }

    #[test]
    fn switching_tabs_keeps_query_and_selects_only_a_matching_agent() {
        crate::test_support::on_large_stack(|| {
            let mut backend = tabbed_backend();
            backend
                .dispatch(Msg::AgentPickerQueryChanged("Claude".into()))
                .unwrap();
            backend.dispatch(Msg::AgentPickerTab(1)).unwrap();
            let picker = backend.state().agent_picker.as_ref().unwrap();
            assert_eq!(picker.input.text(), "Claude");
            assert!(matches!(
                picker.selected,
                Some(AgentLocation::OtherSession { pane: 3, .. })
            ));
            backend.dispatch(Msg::AgentPickerTab(0)).unwrap();
            let picker = backend.state().agent_picker.as_ref().unwrap();
            assert_eq!(picker.input.text(), "Claude");
            assert_eq!(picker.selected, None);
            backend.render();
            assert_eq!(
                backend.state().agent_picker.as_ref().unwrap().selected,
                None
            );
        });
    }

    #[test]
    fn a_session_tab_keeps_its_identity_when_snapshots_change_and_can_be_empty() {
        crate::test_support::on_large_stack(|| {
            let mut backend = tabbed_backend();
            backend.dispatch(Msg::AgentPickerTab(1)).unwrap();
            let active = backend.state().agent_picker.as_ref().unwrap().tab.clone();
            let snapshot = backend.state_mut().local_agent_snapshot.as_mut().unwrap();
            let mut newcomer = snapshot.agents[0].clone();
            newcomer.session = "aaa".into();
            newcomer.reference.pane.pane_id = 7;
            snapshot.agents.push(newcomer);
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            backend.render();
            assert_eq!(picker_tabs(backend.state())[2], active);
            assert_eq!(backend.state().agent_picker.as_ref().unwrap().tab, active);
            backend.state_mut().remote.agents.clear();
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            backend.render();
            assert_eq!(backend.state().agent_picker.as_ref().unwrap().tab, active);
            assert!(picker_tabs(backend.state()).contains(&active));
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(frame.contains("No agents in this session"), "{frame}");
            assert!(!frame.contains("Local Codex"), "{frame}");
            assert_eq!(
                backend.state().agent_picker.as_ref().unwrap().selected,
                None
            );
        });
    }

    /// Two agents of one kind in one session share a label, which a label-keyed palette folds into
    /// a single row. Every refresh then put the cursor back on the first of them.
    #[test]
    fn the_cursor_stays_on_one_of_several_identically_named_agents_across_refreshes() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            let summary = |pane| crate::session::protocol::AgentSummary {
                reference: crate::session::protocol::AgentRef::for_test(pane, 0, None),
                session: "dev".into(),
                agent: "codex".into(),
                label: "Codex".into(),
                state: crate::session::protocol::AgentState::from_status("idle"),
                reason: None,
                task: None,
                completions: 0,
                changed_at: 0,
            };
            backend.state_mut().local_agent_snapshot =
                Some(crate::session::discovery::LocalAgentSnapshot {
                    projects: Vec::new(),
                    sessions: Vec::new(),
                    agents: vec![summary(1), summary(2), summary(3)],
                });
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
                .unwrap();
            backend.render();
            let down = KeyEvent {
                code: KeyCode::Down,
                mods: KeyMods::NONE,
            };
            let selected_pane = |backend: &TestBackend<AppRoot>| {
                backend
                    .state()
                    .agent_picker
                    .as_ref()
                    .and_then(|picker| picker.selected.as_ref())
                    .map(AgentLocation::pane)
            };
            backend.send_key(down).unwrap();
            backend.render();
            assert_eq!(selected_pane(&backend), Some(2));
            // A poll that brings another row is a refresh of the palette's items.
            backend
                .state_mut()
                .local_agent_snapshot
                .as_mut()
                .unwrap()
                .agents
                .push(summary(4));
            backend.dispatch(Msg::RefreshPaintLayers).unwrap();
            backend.render();
            backend.send_key(down).unwrap();
            backend.render();
            assert_eq!(selected_pane(&backend), Some(3));
        });
    }

    /// The rendered Agents rows for `agents` (label, session) on a 100-column screen.
    fn rendered_rows(agents: &[(&str, &str)]) -> Vec<String> {
        let agents = agents
            .iter()
            .enumerate()
            .map(
                |(pane, (label, session))| crate::session::protocol::AgentSummary {
                    reference: crate::session::protocol::AgentRef::for_test(
                        pane as crate::state::PaneId + 1,
                        0,
                        None,
                    ),
                    session: (*session).into(),
                    agent: "codex".into(),
                    label: (*label).into(),
                    state: crate::session::protocol::AgentState::from_status("idle"),
                    reason: None,
                    task: None,
                    completions: 0,
                    changed_at: 0,
                },
            )
            .collect::<Vec<_>>();
        let mut backend = backend_with_panes(&[]);
        backend.state_mut().config.animations.picker =
            crate::layout::anim::PickerAnimationStyle::Off;
        backend.state_mut().local_agent_snapshot =
            Some(crate::session::discovery::LocalAgentSnapshot {
                projects: Vec::new(),
                sessions: Vec::new(),
                agents,
            });
        backend
            .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
            .unwrap();
        backend.render();
        backend
            .capture_frame()
            .to_fixed_grid_lines()
            .into_iter()
            .filter(|line| line.contains("Codex"))
            .collect()
    }

    /// Character column of `needle` in a rendered `row`.
    fn column(row: &str, needle: &str) -> usize {
        let byte = row
            .find(needle)
            .unwrap_or_else(|| panic!("{needle:?} in {row:?}"));
        row[..byte].chars().count()
    }

    /// A short description stays right-aligned like every picker's; a long one starts two columns
    /// after the name instead of at a fixed column. When both sides are long the label gives way
    /// down to a third of the row, and a long label beside a short description is cut only as far
    /// as the description needs.
    #[test]
    fn row_descriptions_follow_the_name_and_share_the_row_with_long_labels() {
        crate::test_support::on_large_stack(|| {
            let long_session = "s".repeat(120);
            let long_label = format!("Codex {}", "l".repeat(120));

            let short = rendered_rows(&[("Codex", "dev")]).remove(0);
            assert!(
                column(&short, "dev") - column(&short, "Codex") > 40,
                "a short description stays at the right edge: {short}"
            );

            let long = rendered_rows(&[("Codex", &long_session)]).remove(0);
            assert!(long.contains("Codex  sss"), "{long}");
            assert!(long.contains("s… │"), "truncated at the end: {long}");

            // The label keeps a third of what the row draws between its borders.
            let both = rendered_rows(&[(&long_label, &long_session)]).remove(0);
            assert!(both.contains("…  sss"), "{both}");
            let row_width = column(&both, " │") - column(&both, "│ ");
            let label_width = column(&both, "…  sss") + 1 - column(&both, "Codex");
            assert!(
                (row_width * 30 / 100..=row_width * 36 / 100).contains(&label_width),
                "{label_width} of {row_width}: {both}"
            );

            let label = rendered_rows(&[(&long_label, "dev")]).remove(0);
            assert!(label.contains("…  dev │"), "{label}");
            assert!(
                column(&label, "…  dev") + 1 - column(&label, "Codex") > row_width * 36 / 100,
                "{label}"
            );
        });
    }

    /// An empty list says which machines were asked. "Nothing running" on a client watching four
    /// hosts is a different fact from the same words on a client watching none.
    #[test]
    fn an_empty_view_says_how_far_it_looked() {
        crate::test_support::on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend.state_mut().config.animations.picker =
                crate::layout::anim::PickerAnimationStyle::Off;
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
                .expect("open the agents view");
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame.contains("No agents running here"),
                "nothing anywhere, and nowhere else was asked:\n{frame}"
            );
        });
    }

    /// A destination fires for the session it named and for no other. Without this, an attach that
    /// went somewhere else — the user changed their mind mid-connect — would drag the cursor onto
    /// whatever pane happened to share the recorded number.
    #[test]
    fn a_destination_waits_for_its_own_session() {
        let mut state =
            crate::state::State::new(crate::config::Config::default(), Theme::default());
        let target = RemoteTarget::Alias("workbox".into());
        state.pending_agent_jump = Some(PendingAgentJump {
            target: Some(target.clone()),
            session: "backend".into(),
            pane: 5,
            row: None,
        });
        assert!(!pending_agent_has_arrived(&state));

        state.current_mut().session_name = Some("web".into());
        state.current_mut().remote_target = Some(target.clone());
        assert!(!pending_agent_has_arrived(&state));

        // The same name on the wrong machine is a different session, not this one.
        state.current_mut().session_name = Some("backend".into());
        state.current_mut().remote_target = None;
        assert!(!pending_agent_has_arrived(&state));

        state.current_mut().remote_target = Some(target);
        assert!(pending_agent_has_arrived(&state));
    }
}
