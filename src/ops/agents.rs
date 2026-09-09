//! The global Agents view: open it, move its cursor, and land on what it points at.
//!
//! Everything the view lists is derived state — live panes for the session on screen, host-monitor
//! snapshots for everything else — so there is nothing here to build or refresh. What this module
//! owns is the one act the view exists for: turning "Codex is blocked on workbox/backend" into
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
    match location {
        AgentLocation::Here { pane, row } => {
            // Landed on before the overlay closes, so the focus the close returns to the "current"
            // pane is the pane the user just asked for rather than the one they came from.
            land_on_pane(ctx, pane, row.as_deref());
            close_agent_picker(ctx)
        }
        AgentLocation::Elsewhere {
            target,
            session,
            pane,
            row,
        } => {
            let Some(entry) = ctx
                .state
                .host_live_sessions
                .iter()
                .find(|entry| {
                    entry.name == session && entry.remote_target.as_ref() == Some(&target)
                })
                .cloned()
            else {
                // Session rows and agent summaries come from the same monitor poll, so a summary
                // with no session row beside it means the host has since stopped reporting it.
                return Update::none();
            };
            ctx.state.pending_agent_jump = Some(PendingAgentJump {
                target: Some(target),
                session,
                pane,
                row,
            });
            let update = crate::ops::session::activate_discovered_session(ctx, entry);
            // Installing a session dismisses the pickers that led into it, so an overlay still
            // standing here means the activation refused (an incompatible server, say) and left
            // its reason on screen. Nothing is coming, so the destination is dropped rather than
            // left to fire on some unrelated later attach to the same name.
            if ctx.state.agent_picker.is_some() {
                ctx.state.pending_agent_jump = None;
                return update;
            }
            // A session already parked in the background switches in synchronously, so its panes
            // are on screen by the time this returns and there is no attach to wait for.
            if land_on_pending_agent(ctx) {
                return Update::full();
            }
            update
        }
    }
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
    land_on_pane(ctx, jump.pane, jump.row.as_deref())
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
fn land_on_pane(ctx: &mut Context<AppRoot>, pane: crate::state::PaneId, row: Option<&str>) -> bool {
    let Some(row) = row else {
        return crate::ops::focus::focus_pane_anywhere(ctx, pane);
    };
    if crate::pane::lifecycle::find_pane_mut(&mut ctx.state, pane).is_none() {
        return false;
    }
    crate::update::sidebar::activation::activate_published_row(ctx, pane, row.to_string());
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

    fn on_large_stack(body: impl FnOnce() + Send + 'static) {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(body)
            .expect("spawn")
            .join()
            .expect("join");
    }

    /// An agent in the session already on screen is a focus change and nothing else — no attach, no
    /// round trip, and the overlay is gone by the time the pane has the keyboard.
    #[test]
    fn a_row_in_this_session_goes_straight_to_its_pane() {
        on_large_stack(|| {
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
        on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend
                .dispatch(Msg::AgentPickerActivate(AgentLocation::Elsewhere {
                    target: RemoteTarget::Alias("workbox".into()),
                    session: "backend".into(),
                    pane: 7,
                    row: None,
                }))
                .expect("activate");
            assert!(backend.state().pending_agent_jump.is_none());
        });
    }

    /// The view's reason to exist, end to end: an agent on a machine this client is not looking at
    /// is on screen, named by host and session, with the state that makes it worth going to.
    #[test]
    fn the_overlay_lists_an_agent_on_another_machine() {
        on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
            backend.state_mut().host_agents.insert(
                RemoteTarget::Alias("workbox".into()),
                vec![crate::session::protocol::AgentSummary {
                    session: "backend".into(),
                    pane: 4,
                    generation: 0,
                    row: None,
                    agent: "codex".into(),
                    label: "Codex".into(),
                    state: "blocked".into(),
                    changed_at: 0,
                }],
            );
            backend
                .dispatch(Msg::RunAction(crate::input::Action::OpenAgentPicker))
                .expect("open the agents view");
            backend.render();
            let frame = backend.capture_frame().to_fixed_grid_lines().join("\n");
            assert!(
                frame.contains("Codex · workbox/backend"),
                "the row names who and where:\n{frame}"
            );
            assert!(
                frame.contains("Blocked"),
                "and what it is waiting on:\n{frame}"
            );
        });
    }

    /// An empty list says which machines were asked. "Nothing running" on a client watching four
    /// hosts is a different fact from the same words on a client watching none.
    #[test]
    fn an_empty_view_says_how_far_it_looked() {
        on_large_stack(|| {
            let mut backend = backend_with_panes(&[]);
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
