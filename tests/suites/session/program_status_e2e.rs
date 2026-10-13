//! Real PTY, multi-client, detach/reattach and keep-open protocol coverage.
use crate::common::{TestConnection, attach_client, read_until, spawn_listener};
use rozi::pane::launch::PaneLaunch;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{ClientMessage, Frame, ServerMessage, WirePalette};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;
use tui_lipan::{ProgramStatusSnapshot, ProgramStatusState};

fn spawn(client: &mut TestConnection, id: u32, command: &str, keep_open: bool) {
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    client.write_control(&ClientMessage::SpawnPane {
        local: false,
        pane_id: id,
        generation: 1,
        launch: Some(PaneLaunch::shell(command.to_string())),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open,
        env: vec![],
        title: Some("OSC test".into()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
        cell_width: 0,
        cell_height: 0,
    });
    read_until(client, |f| {
        if let Frame::Control(ServerMessage::SpawnResult {
            pane_id, ok, error, ..
        }) = f
            && *pane_id == id
        {
            assert!(*ok, "spawn failed: {error:?}");
        }
        matches!(f, Frame::Control(ServerMessage::SpawnResult {pane_id, ok: true, ..}) if *pane_id == id)
    });
}

fn snapshot(
    client: &mut TestConnection,
    id: u32,
    state: ProgramStatusState,
) -> ProgramStatusSnapshot {
    let mut result = None;
    read_until(client, |frame| {
        if let Frame::Control(ServerMessage::ProgramStatusChanged {
            pane_id, snapshot, ..
        }) = frame
            && *pane_id == id
            && snapshot.records.iter().any(|r| r.state == state)
        {
            result = Some(snapshot.clone());
            return true;
        }
        false
    });
    result.unwrap()
}

#[test]
fn program_status_real_pty_multi_client_detached_output_and_keep_open() {
    let server = spawn_listener(ServerSettings {
        allow_takeover: false,
        ..Default::default()
    });
    let (mut a, _) = attach_client(server.endpoint(), server.session(), "first window");
    let (mut b, _) = attach_client(server.endpoint(), server.session(), "second window");
    spawn(
        &mut a,
        81,
        r"printf '\033]7501;state=working:app=codex\007'; read answer; printf '\033]7501;state=error:msg=ZmFpbGVk\033\\'; read answer",
        true,
    );
    let first = snapshot(&mut a, 81, ProgramStatusState::Working);
    assert_eq!(snapshot(&mut b, 81, ProgramStatusState::Working), first);
    for c in [&mut a, &mut b] {
        c.write_control(&ClientMessage::Detach);
    }
    drop(a);
    drop(b);
    // The command keeps running with no UI attached; reattach does not reconstruct from output.
    let (mut c, attached) = attach_client(server.endpoint(), server.session(), "reattached");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!()
    };
    let pane = panes.iter().find(|p| p.pane_id == 81).unwrap();
    assert_eq!(pane.program_status, first);
    assert!(pane.runtime.detected_agent.is_none());
    c.write_pane_input(81, 1, b"continue\n");
    let failed = snapshot(&mut c, 81, ProgramStatusState::Error);
    assert_eq!(failed.records[0].msg.as_deref(), Some("failed"));
    c.write_pane_input(81, 1, b"exit\n");
    // Wait for the replacement shell's runtime metadata, not a fixed sleep.
    read_until(
        &mut c,
        |f| matches!(f, Frame::PaneBytes {pane_id:81,bytes,..} if crate::common::contains(bytes, b"command exited with status 0")),
    );
    let (mut d, attached) = attach_client(server.endpoint(), server.session(), "after replacement");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!()
    };
    assert_eq!(
        panes
            .iter()
            .find(|p| p.pane_id == 81)
            .unwrap()
            .program_status,
        failed
    );
    // A second pane can use the same root ID independently; its active record clears at exit.
    spawn(
        &mut c,
        82,
        r"printf '\033]7501;state=blocked:kind=permission\007'; read answer",
        true,
    );
    snapshot(&mut c, 82, ProgramStatusState::Blocked);
    c.write_pane_input(82, 1, b"exit\n");
    read_until(
        &mut c,
        |f| matches!(f, Frame::Control(ServerMessage::ProgramStatusChanged {pane_id:82,snapshot,..}) if snapshot.records.is_empty()),
    );
    let (mut e, attached) = attach_client(server.endpoint(), server.session(), "final state");
    let ServerMessage::Attached { panes, .. } = attached else {
        panic!()
    };
    assert_eq!(
        panes
            .iter()
            .find(|p| p.pane_id == 81)
            .unwrap()
            .program_status,
        failed
    );
    assert!(
        panes
            .iter()
            .find(|p| p.pane_id == 82)
            .unwrap()
            .program_status
            .records
            .is_empty()
    );
    for client in [&mut c, &mut d, &mut e] {
        client.write_control(&ClientMessage::Detach);
    }
}
