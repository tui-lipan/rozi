use tui_lipan::prelude::CommandLink;

use crate::Msg;

use super::protocol::{Frame, ServerMessage};

/// How a launch begins its session: either attach straight to a session, or show the startup
/// picker and defer attaching until the user chooses.
pub(crate) enum SessionStart {
    Attach {
        epoch: u64,
        name: String,
        autostart: bool,
        create_only: bool,
    },
    Picker {
        epoch: u64,
    },
}

/// Whether any *named* (non-ephemeral) session is currently discoverable. Used to gate the startup
/// picker: with no named session to reattach to, a bare launch skips the picker and attaches to an
/// ephemeral session as usual.
pub(crate) fn has_named_session() -> bool {
    super::discovery::discover_sessions()
        .map(|rows| rows.iter().any(|row| !row.ephemeral))
        .unwrap_or(false)
}

pub(crate) fn attach_session_client(
    epoch: u64,
    name: String,
    autostart: bool,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, autostart, read_only, false, None, link);
}

pub(crate) fn create_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, true, read_only, true, None, link);
}

pub(crate) fn attach_remote_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    remote: super::remote::RemoteTarget,
    remote_config: crate::config::HyprmuxRemoteConfig,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(
        epoch,
        name,
        true,
        read_only,
        create_only,
        Some((remote, remote_config)),
        link,
    );
}

fn attach_session_client_with_profile(
    epoch: u64,
    name: String,
    autostart: bool,
    read_only: bool,
    create_only: bool,
    remote: Option<(
        super::remote::RemoteTarget,
        crate::config::HyprmuxRemoteConfig,
    )>,
    link: CommandLink<Msg>,
) {
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    if let Some((target, remote_config)) = remote {
        attach_remote(
            epoch,
            name,
            read_only,
            create_only,
            target,
            remote_config,
            link,
        );
        return;
    }

    let Ok(path) = super::server::session_socket_path(&name) else {
        link.send(Msg::SessionAttachFailed {
            epoch,
            message: format!("Invalid session name `{name}`"),
        });
        return;
    };
    let endpoint = crate::platform::ipc::IpcEndpoint::at_path(&path);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut spawned = false;
    let mut server_child: Option<std::process::Child> = None;
    loop {
        let (tx, rx) = mpsc::channel();
        match super::client::SessionClient::connect_attached(&endpoint, name.clone(), tx, read_only)
        {
            Ok((client, attached)) => {
                if create_only && !spawned {
                    client.detach();
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}` is already running"),
                    });
                    return;
                }
                if create_only {
                    let expected = server_child.as_ref().map(std::process::Child::id);
                    if expected.is_none() || client.server_pid() != expected {
                        client.detach();
                        link.send(Msg::SessionAttachFailed {
                            epoch,
                            message: format!("Session `{name}` was created by another process"),
                        });
                        return;
                    }
                }
                link.send(Msg::SessionConnected {
                    epoch,
                    name: name.clone(),
                    client,
                });
                link.send(server_message_to_msg(epoch, Frame::Control(attached)));
                for message in rx {
                    link.send(server_message_to_msg(epoch, message));
                }
                link.send(Msg::SessionDisconnected { epoch, name });
                return;
            }
            Err(err) => {
                if is_busy_attach_error(&err) {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}` is busy or not accepting clients"),
                    });
                    return;
                }
                if is_handshake_rejected(&err) {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}`: {err}"),
                    });
                    return;
                }
                if !autostart && should_autostart_session(&err) {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Session `{name}` is not running"),
                    });
                    return;
                }
                if !spawned && should_autostart_session(&err) {
                    spawned = true;
                    if path.exists() {
                        let _ = std::fs::remove_file(&path);
                    }
                    let exe = match std::env::current_exe() {
                        Ok(exe) => exe,
                        Err(exe_err) => {
                            link.send(Msg::SessionAttachFailed {
                                epoch,
                                message: format!(
                                    "Could not start server for `{name}`: unable to locate hyprmux executable: {exe_err}"
                                ),
                            });
                            return;
                        }
                    };
                    // An updated/rebuilt binary unlinks the one this client runs from; on Linux
                    // `current_exe` then points at `hyprmux (deleted)`, which cannot be spawned.
                    // Name the real cause instead of surfacing a raw ENOENT.
                    if !exe.exists() {
                        link.send(Msg::SessionAttachFailed {
                            epoch,
                            message:
                                "hyprmux was updated on disk\nRestart it to start new sessions"
                                    .to_string(),
                        });
                        return;
                    }
                    match crate::platform::server_lifecycle::spawn_detached_server(
                        &exe,
                        &name,
                        create_only,
                    ) {
                        Ok(child) => server_child = Some(child),
                        Err(spawn_err) => {
                            link.send(Msg::SessionAttachFailed {
                                epoch,
                                message: format!(
                                    "Could not start server for `{name}` ({}): {spawn_err}",
                                    exe.display()
                                ),
                            });
                            return;
                        }
                    }
                }
                let early_exit = server_child
                    .as_mut()
                    .and_then(|child| child.try_wait().ok().flatten());
                if Instant::now() >= deadline || early_exit.is_some() {
                    let detail = match early_exit {
                        Some(status) => {
                            format!("session server exited before it was ready ({status})")
                        }
                        None => err.to_string(),
                    };
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Could not attach to `{name}`: {detail}"),
                    });
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

fn attach_remote(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    target: super::remote::RemoteTarget,
    remote_config: crate::config::HyprmuxRemoteConfig,
    link: CommandLink<Msg>,
) {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel();
    match super::remote::connect_remote(&target, &name, &remote_config) {
        Ok((stream, preamble)) => {
            if create_only && !preamble.server_started {
                // Drop the pipe without attaching — session already existed remotely.
                drop(stream);
                link.send(Msg::SessionAttachFailed {
                    epoch,
                    message: format!("Session `{name}` is already running on the remote host"),
                });
                return;
            }
            match super::client::SessionClient::from_stream_attached(
                stream,
                name.clone(),
                tx,
                read_only,
            ) {
                Ok((client, attached)) => {
                    link.send(Msg::SessionConnected {
                        epoch,
                        name: name.clone(),
                        client,
                    });
                    link.send(server_message_to_msg(epoch, Frame::Control(attached)));
                    for message in rx {
                        link.send(server_message_to_msg(epoch, message));
                    }
                    link.send(Msg::SessionDisconnected { epoch, name });
                }
                Err(err) => {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Remote session `{name}`: {err}"),
                    });
                }
            }
        }
        Err(err) => {
            link.send(Msg::SessionAttachFailed {
                epoch,
                message: format!("Remote attach to `{name}` failed: {err}"),
            });
        }
    }
}

fn should_autostart_session(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
    )
}

fn is_busy_attach_error(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    )
}

fn is_handshake_rejected(err: &std::io::Error) -> bool {
    err.kind() == std::io::ErrorKind::InvalidData
}

fn server_message_to_msg(epoch: u64, frame: Frame<ServerMessage>) -> Msg {
    match frame {
        Frame::PaneBytes {
            pane_id,
            generation,
            bytes,
        } => Msg::SessionOutput {
            epoch,
            pane_id,
            generation,
            bytes,
        },
        Frame::Control(message) => match message {
            ServerMessage::Attached {
                session,
                client_id,
                panes,
                layout_rev,
                layout,
                controller,
                clients,
                input_locked,
                created_from_profile,
                ..
            } => Msg::SessionAttached {
                epoch,
                session,
                client_id,
                panes,
                layout_rev,
                layout,
                controller,
                read_only: clients
                    .iter()
                    .find(|client| client.id == client_id)
                    .is_some_and(|client| client.read_only),
                clients,
                input_locked,
                created_from_profile,
            },
            ServerMessage::SessionInfo { .. } => Msg::SessionError {
                epoch,
                message: String::new(),
            },
            ServerMessage::SessionOriginSet {
                created_from_profile,
            } => Msg::SessionOriginSet {
                epoch,
                created_from_profile,
            },
            ServerMessage::LayoutCommitted {
                rev,
                author,
                layout,
            } => Msg::SessionLayoutCommitted {
                epoch,
                rev,
                author,
                layout,
            },
            ServerMessage::LayoutRejected {
                current_rev,
                layout,
            } => Msg::SessionLayoutRejected {
                epoch,
                current_rev,
                layout,
            },
            ServerMessage::ControllerChanged { controller, reason } => {
                Msg::SessionControllerChanged {
                    epoch,
                    controller,
                    reason,
                }
            }
            ServerMessage::ClientsChanged {
                clients,
                input_locked,
            } => Msg::SessionClientsChanged {
                epoch,
                clients,
                input_locked,
            },
            ServerMessage::ControlRequested { from } => {
                Msg::SessionControlRequested { epoch, from }
            }
            ServerMessage::ControlDeclined => Msg::SessionControlDeclined { epoch },
            ServerMessage::Ping { seq } => Msg::SessionPing { epoch, seq },
            ServerMessage::Resized {
                pane_id,
                generation,
                cols,
                rows,
            } => Msg::SessionResized {
                epoch,
                pane_id,
                generation,
                cols,
                rows,
            },
            ServerMessage::Exited {
                pane_id,
                generation,
                code,
            } => Msg::SessionExited {
                epoch,
                pane_id,
                generation,
                code,
            },
            ServerMessage::SpawnResult {
                pane_id,
                generation,
                pid,
                ok,
                error,
            } => Msg::SessionSpawnResult {
                epoch,
                pane_id,
                generation,
                pid,
                ok,
                error,
            },
            ServerMessage::Error { message, .. } => Msg::SessionError { epoch, message },
            ServerMessage::Renamed { session } => Msg::SessionRenamed { epoch, session },
            ServerMessage::PaneLoggingChanged {
                pane_id,
                generation,
                enabled,
                path,
                error,
            } => Msg::SessionPaneLoggingChanged {
                epoch,
                pane_id,
                generation,
                enabled,
                path,
                error,
            },
            ServerMessage::PaneRuntimeChanged {
                pane_id,
                generation,
                state,
            } => Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id,
                generation,
                state,
            },
        },
    }
}
