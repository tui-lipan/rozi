//! End-to-end coverage for published agent slots.
//!
//! Talks to the real session server through the production typed protocol, so this exercises the
//! server's own run-clock bookkeeping rather than a reimplementation of it.

mod common;

use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    AgentSlot, ClientMessage, Frame, PaneRuntimeState, ServerMessage, WirePalette,
};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use common::{attach_client, read_until, spawn_listener};

const PANE_ID: u32 = 52;
const PANE_GENERATION: u64 = 1;

fn slot(id: &str, status: &str, active: bool) -> AgentSlot {
    AgentSlot {
        id: id.to_string(),
        title: format!("tab {id}"),
        status: status.to_string(),
        reason: None,
        active,
        work_started_at: None,
    }
}

fn spawn_pane(controller: &mut common::TestConnection) {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    controller.write_control(&ClientMessage::SpawnPane {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        command: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: Some("agent slots e2e".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(controller, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::SpawnResult {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                ok: true,
                ..
            })
        )
    });
}

/// Wait for the next runtime broadcast whose slots satisfy `check`, and return that state.
fn read_slots(
    client: &mut common::TestConnection,
    check: impl Fn(&[AgentSlot]) -> bool,
) -> PaneRuntimeState {
    let mut captured = None;
    read_until(client, |frame| match frame {
        Frame::Control(ServerMessage::PaneRuntimeChanged {
            pane_id: PANE_ID,
            state,
            ..
        }) if check(&state.slots) => {
            captured = Some(state.clone());
            true
        }
        _ => false,
    });
    captured.expect("read_until matched without capturing")
}

#[test]
fn published_slots_broadcast_and_keep_a_run_clock_per_slot() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    controller.write_control(&ClientMessage::ReportPaneSlots {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        slots: vec![slot("a", "working", true), slot("b", "idle", false)],
    });

    // Both clients see the slots; the server stamps a run start for the working one only.
    let mut started = None;
    for client in [&mut controller, &mut follower] {
        let state = read_slots(client, |slots| slots.len() == 2);
        assert_eq!(state.slots[0].id, "a");
        assert_eq!(state.slots[1].id, "b");
        let a = state.slots[0].work_started_at.expect("a is working");
        assert_eq!(
            state.slots[1].work_started_at, None,
            "an idle slot has no run"
        );
        started = Some(a);
    }
    let started = started.expect("at least one client read the slots");

    // Reordering and retitling must not restart a run: identity is the publisher's id.
    controller.write_control(&ClientMessage::ReportPaneSlots {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        slots: vec![
            slot("b", "working", false),
            AgentSlot {
                title: "renamed".to_string(),
                ..slot("a", "working", true)
            },
        ],
    });
    let state = read_slots(&mut controller, |slots| {
        slots.first().is_some_and(|slot| slot.id == "b")
    });
    let a = state
        .slots
        .iter()
        .find(|slot| slot.id == "a")
        .expect("slot a survived the reorder");
    assert_eq!(
        a.work_started_at,
        Some(started),
        "reordering and retitling must not restart a run"
    );
    assert_eq!(a.title, "renamed");
    assert!(
        state
            .slots
            .iter()
            .any(|slot| slot.id == "b" && slot.work_started_at.is_some_and(|at| at >= started)),
        "b started its own run when it began working"
    );

    // A blocked slot keeps its run: blocking and resuming are one run, as for a whole pane.
    controller.write_control(&ClientMessage::ReportPaneSlots {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        slots: vec![slot("a", "blocked", true)],
    });
    let state = read_slots(&mut controller, |slots| slots.len() == 1);
    assert_eq!(state.slots[0].work_started_at, Some(started));
    assert!(
        !state.slots.iter().any(|slot| slot.id == "b"),
        "a slot the publisher dropped is gone"
    );

    // Withdrawing hands the pane back to screen detection.
    controller.write_control(&ClientMessage::ReportPaneSlots {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        slots: Vec::new(),
    });
    let state = read_slots(&mut controller, |slots| slots.is_empty());
    assert!(state.slots.is_empty());

    follower.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}

/// Slots ride the attach snapshot, so a client that joins mid-run sees them without a broadcast.
#[test]
fn published_slots_survive_detach_and_reattach() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    controller.write_control(&ClientMessage::ReportPaneSlots {
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        slots: vec![slot("only", "working", true)],
    });
    read_slots(&mut controller, |slots| slots.len() == 1);

    let (mut reattached, attached) =
        attach_client(server.endpoint(), server.session(), "reattached");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!("expected attached response");
    };
    let pane = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("reattach response omitted the live pane");
    assert_eq!(pane.runtime.slots.len(), 1);
    assert_eq!(pane.runtime.slots[0].id, "only");
    assert!(pane.runtime.slots[0].work_started_at.is_some());

    reattached.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}
