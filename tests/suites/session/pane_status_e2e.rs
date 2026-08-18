//! End-to-end coverage for server-owned per-pane status.
//!
//! This talks directly to the real session server through the production typed protocol. It
//! deliberately sets status from a writable follower: status updates are not restricted to the
//! layout controller.

use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{ClientMessage, Frame, ServerMessage, WirePalette};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{attach_client, read_until, spawn_listener};

const PANE_ID: u32 = 41;
const PANE_GENERATION: u64 = 1;

#[test]
fn pane_status_broadcasts_and_survives_detach_reattach() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    controller.write_control(&ClientMessage::SpawnPane {
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: None,
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: Vec::new(),
        title: Some("pane status e2e".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(&mut controller, |frame| {
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

    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    follower.write_control(&ClientMessage::SetPaneStatus {
        pane_id: PANE_ID,
        local: false,
        generation: PANE_GENERATION,
        status: Some("blocked".to_string()),
        reason: Some("needs approval".to_string()),
    });

    for client in [&mut controller, &mut follower] {
        read_until(client, |frame| {
            matches!(
                frame,
                Frame::Control(ServerMessage::PaneRuntimeChanged {
                    pane_id: PANE_ID,
            local: false,
                    generation: PANE_GENERATION,
                    state,
                }) if state.status.as_ref().is_some_and(|status| {
                    status.value == "blocked"
                        && status.reason.as_deref() == Some("needs approval")
                        && status.set_at > 0
                })
            )
        });
    }

    follower.write_control(&ClientMessage::Detach);
    drop(follower);

    let (mut reattached, attached) =
        attach_client(server.endpoint(), server.session(), "reattached follower");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!("expected attached response");
    };
    let pane = panes
        .iter()
        .find(|pane| pane.pane_id == PANE_ID && pane.generation == PANE_GENERATION)
        .expect("reattach response omitted the live pane");
    let status = pane
        .runtime
        .status
        .as_ref()
        .expect("reattach response omitted the pane status");
    assert_eq!(status.value, "blocked");
    assert_eq!(status.reason.as_deref(), Some("needs approval"));
    assert!(status.set_at > 0);

    reattached.write_control(&ClientMessage::Detach);
    controller.write_control(&ClientMessage::Detach);
}
