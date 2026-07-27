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

/// Whether a bare launch has anything worth picking from, which is what decides between opening the
/// startup picker and going straight to an ephemeral session. Broader than [`has_named_session`]:
/// a restorable snapshot or a remote host we have seen sessions on is just as pickable as a locally
/// running one, and the picker's remote rows come from that cache before any probe completes.
pub(crate) fn has_session_candidates() -> bool {
    has_named_session()
        || !super::server::list_snapshot_names_by_recency().is_empty()
        || super::read_host_session_cache()
            .values()
            .any(|sessions| !sessions.is_empty())
}

pub(crate) fn attach_session_client(
    epoch: u64,
    name: String,
    autostart: bool,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, autostart, read_only, false, None, false, link);
}

pub(crate) fn create_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, true, read_only, true, None, false, link);
}

/// `reconnect` is true only when re-driving an *established* link that dropped: that path retries
/// with backoff to ride out a suspend/Wi-Fi/VPN blip. The initial startup / attach-elsewhere path
/// passes false so an unreachable host fails after one attempt and the caller can fall back to a
/// local ephemeral instead of leaving the UI blank for the whole retry window.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attach_remote_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    remote: super::remote::RemoteTarget,
    remote_config: crate::config::HyprmuxRemoteConfig,
    reconnect: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(
        epoch,
        name,
        true,
        read_only,
        create_only,
        Some((remote, remote_config)),
        reconnect,
        link,
    );
}

#[allow(clippy::too_many_arguments)]
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
    reconnect: bool,
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
            reconnect,
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

/// How long the remote attach path keeps retrying transient connect failures before giving up. A
/// remote link needs to ride out suspend, Wi-Fi flap, and VPN blips rather than dying on the first
/// failed connect (the disconnect handler re-drives this whole path on an established link that
/// later drops, so this deadline governs the connect phase only).
const REMOTE_RECONNECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);
const REMOTE_RECONNECT_INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
const REMOTE_RECONNECT_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(4);

#[allow(clippy::too_many_arguments)]
fn attach_remote(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    target: super::remote::RemoteTarget,
    remote_config: crate::config::HyprmuxRemoteConfig,
    reconnect: bool,
    link: CommandLink<Msg>,
) {
    use std::time::Instant;

    // Remote autostart lives inside `--remote-serve` on the far side, so there is no local
    // spawn loop here. When re-driving a dropped link (`reconnect`), a transient connect failure is
    // retried with exponential backoff until the deadline, to ride out a suspend/Wi-Fi/VPN blip; the
    // initial attach does not loop, so an unreachable host fails fast and the caller falls back to a
    // local ephemeral. A protocol-version skew is handled separately (kill + one restart) since
    // backing off would never fix a mismatch. `try_attach_remote` sends `SessionConnected`/
    // `SessionAttached` only once it is actually connected, so a failed attempt leaves nothing to
    // undo before the next try.
    let deadline = Instant::now()
        + if reconnect {
            REMOTE_RECONNECT_DEADLINE
        } else {
            std::time::Duration::ZERO
        };
    let mut backoff = REMOTE_RECONNECT_INITIAL_BACKOFF;
    loop {
        match try_attach_remote(
            epoch,
            &name,
            read_only,
            create_only,
            &target,
            &remote_config,
            &link,
        ) {
            AttachRemoteOutcome::Done => return,
            AttachRemoteOutcome::ProtocolSkew(message) => {
                attach_remote_after_skew(
                    epoch,
                    &name,
                    read_only,
                    create_only,
                    &target,
                    &remote_config,
                    &link,
                    message,
                );
                return;
            }
            AttachRemoteOutcome::Fatal(message) => {
                link.send(Msg::SessionAttachFailed {
                    epoch,
                    message: format!("Remote attach to `{name}` failed: {message}"),
                });
                return;
            }
            AttachRemoteOutcome::Failed(message) => {
                if Instant::now() >= deadline {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Remote attach to `{name}` failed: {message}"),
                    });
                    return;
                }
                std::thread::sleep(backoff);
                backoff = (backoff * 2).min(REMOTE_RECONNECT_MAX_BACKOFF);
            }
        }
    }
}

/// Version-skew recovery: kill the incompatible remote server, then try once more. Backing off and
/// retrying the same server would never negotiate, so this path does not loop.
#[allow(clippy::too_many_arguments)]
fn attach_remote_after_skew(
    epoch: u64,
    name: &str,
    read_only: bool,
    create_only: bool,
    target: &super::remote::RemoteTarget,
    remote_config: &crate::config::HyprmuxRemoteConfig,
    link: &CommandLink<Msg>,
    message: String,
) {
    match super::remote::kill_remote_session(target, name, remote_config) {
        Ok(()) => {
            match try_attach_remote(
                epoch,
                name,
                read_only,
                create_only,
                target,
                remote_config,
                link,
            ) {
                AttachRemoteOutcome::Done => {}
                AttachRemoteOutcome::ProtocolSkew(again)
                | AttachRemoteOutcome::Failed(again)
                | AttachRemoteOutcome::Fatal(again) => {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!(
                            "Remote attach to `{name}` failed after restarting an incompatible server: {again}"
                        ),
                    });
                }
            }
        }
        Err(kill_err) => {
            link.send(Msg::SessionAttachFailed {
                epoch,
                message: format!(
                    "Remote attach to `{name}` failed: {message} (restart also failed: {kill_err})"
                ),
            });
        }
    }
}

enum AttachRemoteOutcome {
    Done,
    ProtocolSkew(String),
    /// A transient failure worth retrying with backoff (connect refused, timeout, dropped read).
    Failed(String),
    /// A logical rejection that retrying cannot fix (e.g. `new` against a name already running).
    Fatal(String),
}

fn try_attach_remote(
    epoch: u64,
    name: &str,
    read_only: bool,
    create_only: bool,
    target: &super::remote::RemoteTarget,
    remote_config: &crate::config::HyprmuxRemoteConfig,
    link: &CommandLink<Msg>,
) -> AttachRemoteOutcome {
    use std::sync::mpsc;

    let (tx, rx) = mpsc::channel();
    match super::remote::connect_remote(target, name, remote_config) {
        Ok((stream, preamble)) => {
            if create_only && !preamble.server_started {
                drop(stream);
                return AttachRemoteOutcome::Fatal(format!(
                    "Session `{name}` is already running on the remote host"
                ));
            }
            match super::client::SessionClient::from_stream_attached(
                stream,
                name.to_string(),
                tx,
                read_only,
            ) {
                Ok((client, attached)) => {
                    link.send(Msg::SessionConnected {
                        epoch,
                        name: name.to_string(),
                        client,
                    });
                    link.send(server_message_to_msg(epoch, Frame::Control(attached)));
                    for message in rx {
                        link.send(server_message_to_msg(epoch, message));
                    }
                    link.send(Msg::SessionDisconnected {
                        epoch,
                        name: name.to_string(),
                    });
                    AttachRemoteOutcome::Done
                }
                Err(err) => {
                    let message = err.to_string();
                    if message.to_ascii_lowercase().contains("incompatible")
                        || message.to_ascii_lowercase().contains("protocol")
                    {
                        AttachRemoteOutcome::ProtocolSkew(message)
                    } else {
                        AttachRemoteOutcome::Failed(format!("Remote session `{name}`: {message}"))
                    }
                }
            }
        }
        Err(err) if err.is_protocol_skew() => AttachRemoteOutcome::ProtocolSkew(err.to_string()),
        Err(err) => AttachRemoteOutcome::Failed(err.to_string()),
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
            ServerMessage::DirectoryListing {
                path,
                entries,
                error,
            } => Msg::SessionDirectoryListing {
                epoch,
                path,
                entries,
                error,
            },
            ServerMessage::ChangeListing {
                root,
                changes,
                error,
            } => Msg::SessionChangeListing {
                epoch,
                root,
                changes,
                error,
            },
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
