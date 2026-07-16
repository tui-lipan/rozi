mod common;

use std::time::{Duration, Instant};

use hyprmux::platform::command::{ShellEnv, resolve_launch_argv};
use hyprmux::session::protocol::{
    ClientMessage, ControllerChangeReason, Frame, ServerMessage, WirePalette,
};
use hyprmux::session::server::ServerSettings;
use hyprmux::shared_layout::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use tui_lipan::prelude::TerminalColorPalette;

use common::{attach_client, contains, read_until, spawn_listener};

const PANE_ID: u32 = 71;
const PANE_GENERATION: u64 = 1;

#[test]
fn concurrent_commits_reject_the_stale_base_revision_with_authoritative_layout() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, attached) =
        attach_client(server.endpoint(), server.session(), "controller");
    let (_follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    let controller_id = attached_client_id(&attached);
    let accepted = empty_layout(100);
    let stale = empty_layout(200);

    controller.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: accepted.clone(),
    });
    controller.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: stale,
    });

    let mut committed = false;
    let mut rejected = false;
    read_until(&mut controller, |frame| {
        match frame {
            Frame::Control(ServerMessage::LayoutCommitted {
                rev,
                author,
                layout,
            }) => {
                assert_eq!((*rev, *author, layout), (1, controller_id, &accepted));
                committed = true;
            }
            Frame::Control(ServerMessage::LayoutRejected {
                current_rev,
                layout,
            }) => {
                assert_eq!(*current_rev, 1);
                assert_eq!(layout.as_ref(), Some(&accepted));
                rejected = true;
            }
            _ => {}
        }
        committed && rejected
    });
}

#[test]
fn controller_drop_promotes_oldest_survivor_and_accepts_its_commit() {
    let server = spawn_listener(ServerSettings::default());
    let (controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    let (mut oldest, oldest_attached) =
        attach_client(server.endpoint(), server.session(), "oldest-follower");
    let (mut newest, _) = attach_client(server.endpoint(), server.session(), "newest-follower");
    let oldest_id = attached_client_id(&oldest_attached);

    drop(controller);
    read_until(&mut oldest, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ControllerChanged {
                controller: Some(id),
                reason: ControllerChangeReason::Granted,
            }) if *id == oldest_id
        )
    });

    let layout = empty_layout(123);
    oldest.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: layout.clone(),
    });
    read_until(&mut newest, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout: received,
            }) if *author == oldest_id && received == &layout
        )
    });
}

#[test]
fn heartbeat_expiry_promotes_a_live_follower() {
    let timeout = Duration::from_millis(350);
    let server = spawn_listener(ServerSettings {
        heartbeat_timeout: timeout,
        ..ServerSettings::default()
    });
    let (_controller, _) = attach_client(server.endpoint(), server.session(), "stale-controller");
    let (mut follower, attached) =
        attach_client(server.endpoint(), server.session(), "live-follower");
    let follower_id = attached_client_id(&attached);

    let refresh_at = Instant::now() + timeout / 2;
    while Instant::now() < refresh_at {
        std::thread::sleep(Duration::from_millis(5));
    }
    follower.write_control(&ClientMessage::Pong { seq: 0 });

    read_until(&mut follower, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ControllerChanged {
                controller: Some(id),
                reason: ControllerChangeReason::Expired,
            }) if *id == follower_id
        )
    });

    let layout = empty_layout(144);
    follower.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: layout.clone(),
    });
    read_until(&mut follower, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout: received,
            }) if *author == follower_id && received == &layout
        )
    });
}

#[test]
fn follower_decodes_interleaved_pane_output_and_layout_frames_coherently() {
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, attached) =
        attach_client(server.endpoint(), server.session(), "controller");
    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "follower");
    let controller_id = attached_client_id(&attached);
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
        title: Some("interleaved protocol test".to_string()),
        palette: WirePalette::from(TerminalColorPalette::default()),
        shell,
        command_shell,
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

    let first_marker = b"hyprmux-interleaved-before";
    controller.write_pane_input(
        PANE_ID,
        PANE_GENERATION,
        b"echo hyprmux-interleaved-before\r",
    );
    let mut first_output = Vec::new();
    read_until(&mut follower, |frame| {
        if let Frame::PaneBytes { bytes, .. } = frame {
            first_output.extend_from_slice(bytes);
        }
        contains(&first_output, first_marker)
    });

    let layout = pane_layout();
    controller.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: layout.clone(),
    });
    controller.write_pane_input(
        PANE_ID,
        PANE_GENERATION,
        b"echo hyprmux-interleaved-after\r",
    );

    let second_marker = b"hyprmux-interleaved-after";
    let mut second_output = Vec::new();
    let mut committed = false;
    read_until(&mut follower, |frame| {
        match frame {
            Frame::PaneBytes { bytes, .. } => second_output.extend_from_slice(bytes),
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout: received,
            }) => {
                assert_eq!(*author, controller_id);
                assert_eq!(received, &layout);
                committed = true;
            }
            _ => {}
        }
        committed && contains(&second_output, second_marker)
    });
}

fn attached_client_id(message: &ServerMessage) -> u64 {
    let ServerMessage::Attached { client_id, .. } = message else {
        panic!("expected attached response, got {message:?}");
    };
    *client_id
}

fn empty_layout(cols: u16) -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: cols,
        canvas_rows: 24,
        workspaces: Vec::new(),
    }
}

fn pane_layout() -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        canvas_cols: 80,
        canvas_rows: 23,
        workspaces: vec![SharedWorkspace {
            index: 0,
            name: Some("protocol".to_string()),
            synchronized: false,
            layout: SharedLayoutKind::Dwindle,
            start_axis: SharedSplitAxis::Horizontal,
            split_ratios: Vec::new(),
            tree: Some(SharedTree::Leaf { pane: PANE_ID }),
            panes: vec![SharedPane {
                pane_id: PANE_ID,
                generation: PANE_GENERATION,
                title: Some("interleaved protocol test".to_string()),
                profile_name: None,
                cwd: None,
                command: None,
                replay: false,
                keep_open: false,
                floating: false,
                fullscreen: false,
                rect: None,
            }],
        }],
    }
}
