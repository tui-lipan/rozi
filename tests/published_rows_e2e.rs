//! End-to-end coverage for published activity rows.
//!
//! Talks to the real session server through the production typed protocol, so this exercises the
//! server's own run-clock bookkeeping rather than a reimplementation of it.

mod common;

use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, Frame, PaneRuntimeState, PublishedRow, ServerMessage, WirePalette,
};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use common::{attach_client, read_until, spawn_listener};

const PANE_ID: u32 = 52;
const PANE_GENERATION: u64 = 1;

fn row(id: &str, status: &str, active: bool) -> PublishedRow {
    PublishedRow {
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
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        command: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: Some("published rows e2e".to_string()),
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

/// Wait for the next runtime broadcast whose rows satisfy `check`, and return that state.
fn read_rows(
    client: &mut common::TestConnection,
    check: impl Fn(&[PublishedRow]) -> bool,
) -> PaneRuntimeState {
    let mut captured = None;
    read_until(client, |frame| match frame {
        Frame::Control(ServerMessage::PaneRuntimeChanged {
            pane_id: PANE_ID,
            state,
            ..
        }) if check(&state.rows) => {
            captured = Some(state.clone());
            true
        }
        _ => false,
    });
    captured.expect("read_until matched without capturing")
}

#[test]
fn published_rows_broadcast_and_keep_a_run_clock_per_row() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        rows: vec![row("a", "working", true), row("b", "idle", false)],
    });

    // Both clients see the rows; the server stamps a run start for the working one only.
    let mut started = None;
    for client in [&mut controller, &mut follower] {
        let state = read_rows(client, |rows| rows.len() == 2);
        assert_eq!(state.rows[0].id, "a");
        assert_eq!(state.rows[1].id, "b");
        let a = state.rows[0].work_started_at.expect("a is working");
        assert_eq!(
            state.rows[1].work_started_at, None,
            "an idle row has no run"
        );
        started = Some(a);
    }
    let started = started.expect("at least one client read the rows");

    // Reordering and retitling must not restart a run: identity is the publisher's id.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        rows: vec![
            row("b", "working", false),
            PublishedRow {
                title: "renamed".to_string(),
                ..row("a", "working", true)
            },
        ],
    });
    let state = read_rows(&mut controller, |rows| {
        rows.first().is_some_and(|row| row.id == "b")
    });
    let a = state
        .rows
        .iter()
        .find(|row| row.id == "a")
        .expect("row a survived the reorder");
    assert_eq!(
        a.work_started_at,
        Some(started),
        "reordering and retitling must not restart a run"
    );
    assert_eq!(a.title, "renamed");
    assert!(
        state
            .rows
            .iter()
            .any(|row| row.id == "b" && row.work_started_at.is_some_and(|at| at >= started)),
        "b started its own run when it began working"
    );

    // A blocked row keeps its run: blocking and resuming are one run, as for a whole pane.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        rows: vec![row("a", "blocked", true)],
    });
    let state = read_rows(&mut controller, |rows| rows.len() == 1);
    assert_eq!(state.rows[0].work_started_at, Some(started));
    assert!(
        !state.rows.iter().any(|row| row.id == "b"),
        "a row the publisher dropped is gone"
    );

    // Withdrawing hands the pane back to screen detection.
    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        rows: Vec::new(),
    });
    let state = read_rows(&mut controller, |rows| rows.is_empty());
    assert!(state.rows.is_empty());

    follower.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}

/// Rows ride the attach snapshot, so a client that joins mid-run sees them without a broadcast.
#[test]
fn published_rows_survive_detach_and_reattach() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    spawn_pane(&mut controller);

    controller.write_control(&ClientMessage::ReportPaneRows {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        rows: vec![row("only", "working", true)],
    });
    read_rows(&mut controller, |rows| rows.len() == 1);

    let (mut reattached, attached) =
        attach_client(server.endpoint(), server.session(), "reattached");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!("expected attached response");
    };
    let pane = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID)
        .expect("reattach response omitted the live pane");
    assert_eq!(pane.runtime.rows.len(), 1);
    assert_eq!(pane.runtime.rows[0].id, "only");

    reattached.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}
