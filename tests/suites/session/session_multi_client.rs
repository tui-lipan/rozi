use std::time::{Duration, Instant};
use std::{io::BufRead, io::Write};

use rozi::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedLayoutKind, SharedPane, SharedSplitAxis, SharedTree,
    SharedWorkspace,
};
use rozi::pane::launch::PaneLaunch;
use rozi::platform::command::{ShellEnv, resolve_launch_argv};
use rozi::session::protocol::{
    ClientMessage, ControllerChangeReason, Frame, ServerMessage, WirePalette,
};
use rozi::session::server::ServerSettings;
use tui_lipan::prelude::TerminalColorPalette;

use crate::common::{attach_client, contains, read_until, spawn_listener};

const PANE_ID: u32 = 71;
const PANE_GENERATION: u64 = 1;

#[test]
fn interleaved_pane_child() {
    if std::env::var_os("ROZI_TEST_INTERLEAVED_CHILD").is_none() {
        return;
    }
    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();
    for (input, output) in [
        ("before", "rozi-interleaved-before"),
        ("after", "rozi-interleaved-after"),
    ] {
        assert_eq!(
            lines
                .next()
                .expect("pane input")
                .expect("read pane input")
                .trim_end_matches('\r'),
            input
        );
        println!("{output}");
        std::io::stdout().flush().expect("flush pane output");
    }
}

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
        live: false,
    });
    controller.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: stale,
        live: false,
    });

    let mut committed = false;
    let mut rejected = false;
    read_until(&mut controller, |frame| {
        match frame {
            Frame::Control(ServerMessage::LayoutCommitted {
                rev,
                author,
                layout,
                live: _,
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
        live: false,
    });
    read_until(&mut newest, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout: received,
                live: _,
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
        live: false,
    });
    read_until(&mut follower, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout: received,
                live: _,
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
    let test_binary = std::env::current_exe().expect("session test binary");
    controller.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PANE_ID, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: Some(PaneLaunch::Direct {
            argv: vec![
                test_binary.to_string_lossy().into_owned(),
                "--exact".to_string(),
                "session_multi_client::interleaved_pane_child".to_string(),
                "--nocapture".to_string(),
            ],
        }),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: vec![("ROZI_TEST_INTERLEAVED_CHILD".to_string(), "1".to_string())],
        title: Some("interleaved protocol test".to_string()),
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

    let first_marker = b"rozi-interleaved-before";
    controller.write_pane_input(PANE_ID, PANE_GENERATION, b"before\r");
    let mut first_output = Vec::new();
    read_until(&mut follower, |frame| {
        if let Frame::PaneOutput { bytes, .. } = frame {
            first_output.extend_from_slice(bytes);
        }
        // Only the helper emits this marker, so echoed input cannot satisfy the wait.
        contains(&first_output, first_marker)
    });

    let layout = pane_layout();
    controller.write_control(&ClientMessage::CommitLayout {
        base_rev: 1,
        layout: layout.clone(),
        live: false,
    });
    controller.write_pane_input(PANE_ID, PANE_GENERATION, b"after\r");

    let second_marker = b"rozi-interleaved-after";
    let mut second_output = Vec::new();
    let mut committed = false;
    read_until(&mut follower, |frame| {
        match frame {
            Frame::PaneOutput { bytes, .. } => second_output.extend_from_slice(bytes),
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 2,
                author,
                layout: received,
                live: _,
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

/// The scenario parking exists for, end to end over the real protocol: one client keeps a session
/// open in the background while it works elsewhere, and the next client to attach must get control
/// of it outright rather than joining as a follower of a connection nobody is watching.
#[test]
fn a_parked_client_hands_the_session_to_the_next_attacher() {
    let server = spawn_listener(ServerSettings::default());
    let (mut background, background_attached) =
        attach_client(server.endpoint(), server.session(), "background");
    let background_id = attached_client_id(&background_attached);
    assert_eq!(
        controller_of(&background_attached),
        Some(background_id),
        "the first attacher leads"
    );

    background.write_control(&ClientMessage::SetParked { parked: true });
    read_until(&mut background, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ControllerChanged {
                controller: None,
                reason: ControllerChangeReason::Released,
            })
        )
    });

    let (_arriving, arriving_attached) =
        attach_client(server.endpoint(), server.session(), "arriving");
    let arriving_id = attached_client_id(&arriving_attached);
    assert_eq!(
        controller_of(&arriving_attached),
        Some(arriving_id),
        "attaching to a session that is only parked elsewhere must not make a follower"
    );

    // And the roster says which clients are merely parked, so a UI can tell them apart.
    read_until(&mut background, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ClientsChanged { clients, .. })
                if clients.iter().any(|client| client.id == background_id && client.parked)
                    && clients.iter().any(|client| client.id == arriving_id && !client.parked)
        )
    });
}

/// Coming back to a session left parked reclaims the lease only if it is free. Someone who took it
/// meanwhile keeps it — returning from the background is not a claim on a session in use.
#[test]
fn unparking_never_steals_a_session_someone_else_took() {
    let server = spawn_listener(ServerSettings::default());
    let (mut background, _) = attach_client(server.endpoint(), server.session(), "background");
    let (mut arriving, arriving_attached) =
        attach_client(server.endpoint(), server.session(), "arriving");
    let arriving_id = attached_client_id(&arriving_attached);

    background.write_control(&ClientMessage::SetParked { parked: true });
    read_until(&mut arriving, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ControllerChanged {
                controller: Some(id),
                ..
            }) if *id == arriving_id
        )
    });

    background.write_control(&ClientMessage::SetParked { parked: false });
    // Absence of a steal is proven positively: both clients commit, and the only commit the server
    // accepts is the one from the client that actually holds the lease.
    background.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: empty_layout(111),
        live: false,
    });
    let held = empty_layout(222);
    arriving.write_control(&ClientMessage::CommitLayout {
        base_rev: 0,
        layout: held.clone(),
        live: false,
    });
    read_until(&mut arriving, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::LayoutCommitted {
                rev: 1,
                author,
                layout,
                live: _,
            }) if *author == arriving_id && layout == &held
        )
    });
}

/// When the controller leaves, the lease has to land on a client that is actually using the
/// session. Handing it to a parked one would put control in a window nobody is looking at and lock
/// out the client in front of the user.
#[test]
fn a_leaving_controller_skips_parked_clients_when_the_lease_moves() {
    let server = spawn_listener(ServerSettings::default());
    let (controller, _) = attach_client(server.endpoint(), server.session(), "controller");
    let (mut parked, parked_attached) =
        attach_client(server.endpoint(), server.session(), "parked");
    let (mut active, active_attached) =
        attach_client(server.endpoint(), server.session(), "active");
    let parked_id = attached_client_id(&parked_attached);
    let active_id = attached_client_id(&active_attached);

    parked.write_control(&ClientMessage::SetParked { parked: true });
    read_until(&mut parked, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ClientsChanged { clients, .. })
                if clients.iter().any(|client| client.id == parked_id && client.parked)
        )
    });

    drop(controller);

    read_until(&mut active, |frame| {
        matches!(
            frame,
            Frame::Control(ServerMessage::ControllerChanged {
                controller: Some(id),
                ..
            }) if *id == active_id
        )
    });
}

fn controller_of(message: &ServerMessage) -> Option<u64> {
    let ServerMessage::Attached { controller, .. } = message else {
        panic!("expected attached response, got {message:?}");
    };
    *controller
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
        pane_geometry: Default::default(),
        canvas_cols: cols,
        canvas_rows: 24,
        workspaces: Vec::new(),
    }
}

fn pane_layout() -> SharedLayout {
    SharedLayout {
        version: SHARED_LAYOUT_VERSION,
        pane_geometry: Default::default(),
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
                launch: None,
                replay: false,
                keep_open: false,
                floating: false,
                fullscreen: false,
                rect: None,
                scrollable_width: rozi::state::DEFAULT_SCROLLABLE_WIDTH,
            }],
        }],
    }
}

#[cfg(unix)]
#[test]
fn owned_media_child() {
    use base64::Engine as _;
    let Ok(command) = std::env::var("ROZI_TEST_OWNED_MEDIA_COMMAND") else {
        return;
    };
    let command = base64::engine::general_purpose::STANDARD
        .decode(command)
        .unwrap();
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        if line.trim_end_matches('\r') == "emit" {
            std::io::stdout().write_all(&command).unwrap();
            println!("owned-media-emitted");
            std::io::stdout().flush().unwrap();
        }
    }
}

// This fixture needs a PTY that preserves Kitty APC bytes. The Windows 26100 ConPTY on CI
// drops the APC before it reaches the server, even with console VT processing enabled.
// The media module's platform-neutral worker/relay/replay test covers that path on Windows.
#[cfg(unix)]
#[test]
fn consumed_media_reaches_two_local_clients_and_a_late_remote_over_the_protocol() {
    use base64::Engine as _;
    use tui_lipan::prelude::{GraphicsMediaPolicy, TerminalScreen};
    let server = spawn_listener(ServerSettings::default());
    let (mut controller, _) =
        attach_client(server.endpoint(), server.session(), "media-controller");
    let (mut follower, _) = attach_client(server.endpoint(), server.session(), "media-local");
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tty-graphics-protocol-owned");
    std::fs::write(&path, [255, 0, 0, 255]).unwrap();
    let name =
        base64::engine::general_purpose::STANDARD.encode(path.as_os_str().as_encoded_bytes());
    let command = format!("\x1b_Ga=T,t=t,f=32,s=1,v=1,i=7,p=1,C=1,q=2;{name}\x1b\\");
    let (shell, command_shell) = resolve_launch_argv(None, None, &ShellEnv::from_process());
    controller.write_control(&ClientMessage::SpawnPane {
        placement: Some(crate::common::spawn_placement(
            0,
            &[(PANE_ID, PANE_GENERATION)],
        )),
        local: false,
        pane_id: PANE_ID,
        generation: PANE_GENERATION,
        launch: Some(PaneLaunch::Direct {
            argv: vec![
                std::env::current_exe()
                    .unwrap()
                    .to_string_lossy()
                    .into_owned(),
                "--exact".into(),
                "session_multi_client::owned_media_child".into(),
                "--nocapture".into(),
            ],
        }),
        cwd: None,
        cols: 80,
        rows: 24,
        keep_open: false,
        env: vec![(
            "ROZI_TEST_OWNED_MEDIA_COMMAND".into(),
            base64::engine::general_purpose::STANDARD.encode(command),
        )],
        title: None,
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
                ok: true,
                ..
            })
        )
    });
    controller.write_pane_input(PANE_ID, PANE_GENERATION, b"emit\r");
    let mut adopted = Vec::new();
    for client in [&mut controller, &mut follower] {
        let mut wire = Vec::new();
        let mut receiver = TerminalScreen::new(24, 80, 100);
        receiver.set_image_file_storage_enabled(true);
        receiver.set_image_media_policy(GraphicsMediaPolicy::SHARED);
        let mut acks = Vec::new();
        read_until(client, |frame| {
            if let Frame::PaneOutput {
                pane_id: PANE_ID,
                bytes,
                media,
                generation,
                local,
                ..
            } = frame
            {
                wire.extend_from_slice(bytes);
                if let Some(media) = media {
                    let bindings: Vec<_> = media
                        .files
                        .iter()
                        .map(|binding| {
                            let file = tui_lipan::TerminalImmutableFile::acquire(
                                &binding.path,
                                binding.byte_len,
                                binding.format,
                            )
                            .unwrap();
                            adopted.push(file.clone());
                            tui_lipan::TerminalOutputMedia {
                                command_range: binding.command_range.clone(),
                                file,
                            }
                        })
                        .collect();
                    let prepared = receiver
                        .prepare_immutable_media_input(
                            std::sync::Arc::from(bytes.as_slice()),
                            bindings,
                        )
                        .unwrap();
                    receiver.process_prepared_media_input(prepared).unwrap();
                    acks.push(rozi::session::protocol::AcquisitionAck {
                        pane_id: PANE_ID,
                        generation: *generation,
                        local: *local,
                        sequence: media.acquisition_sequence,
                    });
                } else {
                    receiver.process_bytes(bytes);
                }
            }
            contains(&wire, b"owned-media-emitted")
        });
        assert!(
            !contains(&wire, b"t=t"),
            "a consuming source reached a client"
        );
        assert!(contains(&wire, b"t=f"));
        assert!(!contains(&wire, name.as_bytes()));
        assert!(receiver.has_images());
        assert!(!acks.is_empty(), "the test must exercise media acquisition");
        let mut acknowledgement = Vec::new();
        rozi::session::protocol::write_acquisition_acks(&mut acknowledgement, &acks).unwrap();
        client.write_raw(&acknowledgement);
    }
    assert_eq!(adopted.len(), 2);
    assert_ne!(
        adopted[0].path(),
        adopted[1].path(),
        "clients must own private names"
    );
    assert!(!path.exists());
    let mut remote = crate::common::TestConnection::connect(server.endpoint());
    let mut attach = crate::common::attach_message(server.session(), "late-media-remote");
    if let ClientMessage::Attach {
        shares_filesystem,
        read_only,
        ..
    } = &mut attach
    {
        *shares_filesystem = false;
        *read_only = true;
    }
    remote.write_control(&attach);
    let mut receiver = TerminalScreen::new(24, 80, 100);
    receiver.set_image_media_policy(GraphicsMediaPolicy::NONE);
    read_until(&mut remote, |frame| {
        if let Frame::PaneOutput {
            pane_id: PANE_ID,
            bytes,
            ..
        } = frame
        {
            assert!(!contains(bytes, b"t=t"));
            receiver.process_bytes(bytes);
        }
        receiver.has_images()
    });
}
