use tui_lipan::prelude::CommandLink;

use crate::Msg;

use super::protocol::{Frame, ServerMessage};

/// Reconnect attempts run on detached worker threads, so changing attachment epochs only makes
/// their eventual messages stale; it does not stop a blocked SSH child. Keep a small cancellation
/// roster that the transport checks while waiting for its preamble.
static CANCELLED_REMOTE_ATTACHES: std::sync::Mutex<std::collections::BTreeSet<u64>> =
    std::sync::Mutex::new(std::collections::BTreeSet::new());

pub(crate) fn cancel_remote_attach(epoch: u64) {
    let Ok(mut cancelled) = CANCELLED_REMOTE_ATTACHES.lock() else {
        return;
    };
    cancelled.insert(epoch);
}

pub(crate) fn remote_attach_cancelled(epoch: u64) -> bool {
    CANCELLED_REMOTE_ATTACHES
        .lock()
        .is_ok_and(|cancelled| cancelled.contains(&epoch))
}

pub(crate) fn finish_cancelled_remote_attach(epoch: u64) {
    if let Ok(mut cancelled) = CANCELLED_REMOTE_ATTACHES.lock() {
        cancelled.remove(&epoch);
    }
}

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
    /// `--remote <host>` with a startup policy that chose no session: land in that host's own
    /// launcher (`Sessions · <host>`) rather than this machine's picker, and contact the host once
    /// the runtime is up.
    RemotePicker {
        target: crate::session::remote::RemoteTarget,
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
    attach_session_client_with_profile(epoch, name, autostart, read_only, false, false, link);
}

/// How long a local reconnect keeps retrying a server that is alive but did not answer the
/// handshake in time. Matches the server's heartbeat budget for a busy peer.
const LOCAL_RECONNECT_BUSY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);

/// Re-drive an established local link that dropped. Unlike [`attach_session_client`], a busy
/// server is retried until [`LOCAL_RECONNECT_BUSY_DEADLINE`] before the reconnect is abandoned.
pub(crate) fn reconnect_session_client(
    epoch: u64,
    name: String,
    autostart: bool,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, autostart, read_only, false, true, link);
}

pub(crate) fn create_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    link: CommandLink<Msg>,
) {
    attach_session_client_with_profile(epoch, name, true, read_only, true, false, link);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RemoteAttachMode {
    Initial,
    Recover,
    Recreate,
}

impl RemoteAttachMode {
    fn reconnect(self) -> bool {
        matches!(self, Self::Recover | Self::Recreate)
    }

    fn recover_existing(self) -> bool {
        self == Self::Recover
    }

    fn create_only(self, requested: bool) -> bool {
        requested || self == Self::Recreate
    }
}

/// In-place recovery and recreation both get deadline/cancellation handling. Recovery additionally
/// forbids replacing the original server; explicit recreation deliberately allows a new one.
#[allow(clippy::too_many_arguments)]
pub(crate) fn attach_remote_session_client(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    remote: super::remote::RemoteTarget,
    remote_config: crate::config::RemoteConfig,
    mode: RemoteAttachMode,
    link: CommandLink<Msg>,
) {
    attach_remote(
        epoch,
        name,
        read_only,
        mode.create_only(create_only),
        remote,
        remote_config,
        mode.reconnect(),
        mode.recover_existing(),
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
    reconnect: bool,
    link: CommandLink<Msg>,
) {
    use std::time::{Duration, Instant};

    let Ok(path) = super::server::session_socket_path(&name) else {
        link.send(Msg::SessionAttachFailed {
            epoch,
            message: format!("Invalid session name `{name}`"),
        });
        return;
    };
    let endpoint = crate::platform::ipc::IpcEndpoint::at_path(&path);
    let deadline = Instant::now() + Duration::from_secs(5);
    let reconnect_deadline = Instant::now() + LOCAL_RECONNECT_BUSY_DEADLINE;
    let mut spawned = false;
    let mut server_child: Option<std::process::Child> = None;
    loop {
        let mailbox = super::client::InboundMailbox::new(epoch, name.clone(), link.clone());
        match super::client::SessionClient::connect_attached_mailbox(
            &endpoint,
            name.clone(),
            std::sync::Arc::clone(&mailbox),
            read_only,
        ) {
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
                mailbox.activate();
                return;
            }
            Err(err) => {
                // A server stalled by a large output burst can miss one handshake window and answer
                // the next; a reconnect rides that out instead of abandoning a live session.
                if reconnect && is_busy_attach_error(&err) && Instant::now() < reconnect_deadline {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                }
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
                                    "Could not start server for `{name}`: unable to locate rozi executable: {exe_err}"
                                ),
                            });
                            return;
                        }
                    };
                    // An updated/rebuilt binary unlinks the one this client runs from; on Linux
                    // `current_exe` then points at `rozi (deleted)`, which cannot be spawned.
                    // Name the real cause instead of surfacing a raw ENOENT.
                    if !exe.exists() {
                        link.send(Msg::SessionAttachFailed {
                            epoch,
                            message: "rozi was updated on disk\nRestart it to start new sessions"
                                .to_string(),
                        });
                        return;
                    }
                    match crate::platform::server_lifecycle::spawn_detached_server(
                        &exe,
                        &name,
                        create_only,
                        None,
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
/// later drops, so this deadline governs the connect phase only). Two minutes covers a typical
/// wake-then-VPN sequence; a host that is still down after that stays offline in place. Each
/// attempt is capped by the remaining window so SSH connect and preamble waits cannot outrun it.
const REMOTE_RECONNECT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);
const REMOTE_RECONNECT_INITIAL_BACKOFF: std::time::Duration = std::time::Duration::from_millis(250);
const REMOTE_RECONNECT_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(4);

#[allow(clippy::too_many_arguments)]
fn attach_remote(
    epoch: u64,
    name: String,
    read_only: bool,
    create_only: bool,
    target: super::remote::RemoteTarget,
    remote_config: crate::config::RemoteConfig,
    reconnect: bool,
    recover_existing: bool,
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
    let mut last_error = format!("timed out after {}s", REMOTE_RECONNECT_DEADLINE.as_secs());
    loop {
        if remote_attach_cancelled(epoch) {
            finish_cancelled_remote_attach(epoch);
            return;
        }
        // Each attempt is capped by the time still inside the advertised window, so a single
        // SSH/preamble wait cannot outrun the deadline and then decide whether to try again.
        let budget = if reconnect {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                link.send(Msg::SessionAttachFailed {
                    epoch,
                    message: format!("Remote attach to `{name}` failed: {last_error}"),
                });
                return;
            }
            Some(remaining)
        } else {
            None
        };
        let outcome = try_attach_remote(
            epoch,
            &name,
            read_only,
            create_only,
            &target,
            &remote_config,
            recover_existing,
            budget,
            reconnect.then_some(epoch),
            &link,
        );
        if remote_attach_cancelled(epoch) {
            finish_cancelled_remote_attach(epoch);
            return;
        }
        match outcome {
            AttachRemoteOutcome::Done => return,
            AttachRemoteOutcome::ProtocolSkew(message) => {
                // Restarting here would deliberately destroy the original processes and then seed
                // a replacement from retained client state. That is valid for a new attach, never
                // for recovery of a link that was already established.
                if !may_restart_after_protocol_skew(reconnect) {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!(
                            "Remote attach to `{name}` failed: the original session runs an incompatible Rozi version and cannot be restarted automatically ({message})"
                        ),
                    });
                    return;
                }
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
            AttachRemoteOutcome::Lost(message) => {
                link.send(Msg::SessionLost { epoch, message });
                return;
            }
            AttachRemoteOutcome::Failed(message) => {
                last_error = message;
                if Instant::now() >= deadline {
                    link.send(Msg::SessionAttachFailed {
                        epoch,
                        message: format!("Remote attach to `{name}` failed: {last_error}"),
                    });
                    return;
                }
                let remaining = deadline.saturating_duration_since(Instant::now());
                if !wait_for_remote_retry(epoch, backoff.min(remaining)) {
                    finish_cancelled_remote_attach(epoch);
                    return;
                }
                backoff = (backoff * 2).min(REMOTE_RECONNECT_MAX_BACKOFF);
            }
        }
    }
}

fn wait_for_remote_retry(epoch: u64, duration: std::time::Duration) -> bool {
    let deadline = std::time::Instant::now() + duration;
    loop {
        if remote_attach_cancelled(epoch) {
            return false;
        }
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return true;
        }
        std::thread::park_timeout(remaining.min(std::time::Duration::from_millis(200)));
    }
}

fn may_restart_after_protocol_skew(reconnect: bool) -> bool {
    !reconnect
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
    remote_config: &crate::config::RemoteConfig,
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
                false,
                None,
                None,
                link,
            ) {
                AttachRemoteOutcome::Done => {}
                AttachRemoteOutcome::ProtocolSkew(again)
                | AttachRemoteOutcome::Failed(again)
                | AttachRemoteOutcome::Fatal(again)
                | AttachRemoteOutcome::Lost(again) => {
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
    /// Existing-only recovery proved the original server is gone.
    Lost(String),
}

#[allow(clippy::too_many_arguments)]
fn try_attach_remote(
    epoch: u64,
    name: &str,
    read_only: bool,
    create_only: bool,
    target: &super::remote::RemoteTarget,
    remote_config: &crate::config::RemoteConfig,
    recover_existing: bool,
    budget: Option<std::time::Duration>,
    cancel_epoch: Option<u64>,
    link: &CommandLink<Msg>,
) -> AttachRemoteOutcome {
    let attempt_started = std::time::Instant::now();
    let mailbox = super::client::InboundMailbox::new(epoch, name.to_string(), link.clone());
    match super::remote::connect_remote_within(
        target,
        name,
        remote_config,
        budget,
        cancel_epoch,
        recover_existing,
    ) {
        Ok((stream, preamble)) => {
            if cancel_epoch.is_some_and(remote_attach_cancelled) {
                drop(stream);
                return AttachRemoteOutcome::Failed("reconnect cancelled".to_string());
            }
            if started_server_lacks_identity(
                preamble.server_started,
                preamble.server_nonce.as_deref(),
            ) {
                drop(stream);
                return AttachRemoteOutcome::Fatal(
                    "Remote proxy started a server without an identity proof".to_string(),
                );
            }
            if create_only_rejects_existing(create_only, preamble.server_started) {
                drop(stream);
                return AttachRemoteOutcome::Fatal(format!(
                    "Session `{name}` is already running on the remote host"
                ));
            }
            // Recovery asks the proxy not to autostart. `server_started` remains a defensive check
            // for a version-skewed proxy that still did so.
            if reconnect_found_original_missing(
                recover_existing,
                preamble.server_started,
                preamble.session_missing,
            ) {
                drop(stream);
                return AttachRemoteOutcome::Lost(format!(
                    "the original remote session `{name}` is gone"
                ));
            }
            let handshake_budget =
                budget.map(|budget| budget.saturating_sub(attempt_started.elapsed()));
            if handshake_budget.is_some_and(|remaining| remaining.is_zero()) {
                drop(stream);
                return AttachRemoteOutcome::Failed("reconnect deadline elapsed".to_string());
            }
            let attached = match handshake_budget {
                Some(timeout) => {
                    super::client::SessionClient::from_stream_attached_mailbox_with_timeout(
                        stream,
                        name.to_string(),
                        std::sync::Arc::clone(&mailbox),
                        read_only,
                        false,
                        timeout,
                        cancel_epoch,
                        preamble.server_nonce,
                    )
                }
                None => super::client::SessionClient::from_stream_attached_mailbox(
                    stream,
                    name.to_string(),
                    std::sync::Arc::clone(&mailbox),
                    read_only,
                    false,
                    preamble.server_nonce,
                ),
            };
            match attached {
                Ok((client, attached)) => {
                    link.send(Msg::SessionConnected {
                        epoch,
                        name: name.to_string(),
                        client,
                    });
                    link.send(server_message_to_msg(epoch, Frame::Control(attached)));
                    mailbox.activate();
                    AttachRemoteOutcome::Done
                }
                Err(err) => {
                    let message = err.to_string();
                    if message.contains("different server") {
                        AttachRemoteOutcome::Fatal(format!("Remote session `{name}`: {message}"))
                    } else if message.to_ascii_lowercase().contains("incompatible")
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

/// Recovery either receives an explicit missing marker from an existing-only proxy, or defensively
/// spots a version-skewed proxy that autostarted anyway.
fn reconnect_found_original_missing(
    recover_existing: bool,
    server_started: bool,
    session_missing: bool,
) -> bool {
    recover_existing && (server_started || session_missing)
}

fn create_only_rejects_existing(create_only: bool, server_started: bool) -> bool {
    create_only && !server_started
}

fn started_server_lacks_identity(server_started: bool, server_nonce: Option<&str>) -> bool {
    server_started && server_nonce.is_none()
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

pub(crate) fn server_message_to_msg(epoch: u64, frame: Frame<ServerMessage>) -> Msg {
    match frame {
        Frame::PaneBytes {
            pane_id,
            local,
            generation,
            bytes,
        } => Msg::SessionOutput {
            epoch,
            pane_id,
            local,
            generation,
            bytes,
        },
        Frame::Control(message) => match message {
            ServerMessage::Attached {
                session_instance,
                session,
                client_id,
                panes,
                layout_rev,
                layout,
                controller,
                clients,
                input_locked,
                allow_takeover,
                created_from_profile,
                ..
            } => Msg::SessionAttached {
                epoch,
                session_instance,
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
                allow_takeover,
                created_from_profile,
            },
            // Neither reaches an attached client: a probe and a headless control request each
            // answer their own short-lived connection. Arriving here means the server answered
            // something this client never asked, which is a broken attach rather than a message
            // to act on.
            ServerMessage::SessionInfo { .. } | ServerMessage::SessionControlResult { .. } => {
                Msg::SessionError {
                    epoch,
                    message: String::new(),
                }
            }
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
            ServerMessage::DragChanged { author, drag } => Msg::SessionDragChanged {
                epoch,
                author,
                drag,
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
                allow_takeover,
            } => Msg::SessionClientsChanged {
                epoch,
                clients,
                input_locked,
                allow_takeover,
            },
            ServerMessage::ControlRequested { from } => {
                Msg::SessionControlRequested { epoch, from }
            }
            ServerMessage::ControlDeclined => Msg::SessionControlDeclined { epoch },
            ServerMessage::Ping { seq } => Msg::SessionPing { epoch, seq },
            ServerMessage::RuntimeMetrics { metrics } => {
                Msg::SessionRuntimeMetrics { epoch, metrics }
            }
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
                local,
                generation,
                cols,
                rows,
            } => Msg::SessionResized {
                epoch,
                pane_id,
                local,
                generation,
                cols,
                rows,
            },
            ServerMessage::PaneReset {
                pane_id,
                generation,
                cols,
                rows,
            } => Msg::SessionPaneReset {
                epoch,
                pane_id,
                generation,
                cols,
                rows,
            },
            ServerMessage::Exited {
                pane_id,
                local,
                generation,
                code,
            } => Msg::SessionExited {
                epoch,
                pane_id,
                local,
                generation,
                code,
            },
            ServerMessage::SpawnResult {
                pane_id,
                local,
                generation,
                pid,
                ok,
                error,
            } => Msg::SessionSpawnResult {
                epoch,
                pane_id,
                local,
                generation,
                pid,
                ok,
                error,
            },
            // An eviction arrives as an error with a reserved code, immediately before the server
            // closes the connection. It has to be told apart from a dropped link here: the generic
            // path reconnects, which would walk straight back into the session we were removed from.
            ServerMessage::Error { code, message }
                if code == crate::session::protocol::EVICTED_ERROR_CODE =>
            {
                Msg::SessionEvicted { epoch, message }
            }
            ServerMessage::Error { message, .. } => Msg::SessionError { epoch, message },
            ServerMessage::Renamed { session } => Msg::SessionRenamed { epoch, session },
            ServerMessage::PaneLoggingChanged {
                pane_id,
                local,
                generation,
                enabled,
                path,
                error,
            } => Msg::SessionPaneLoggingChanged {
                epoch,
                pane_id,
                local,
                generation,
                enabled,
                path,
                error,
            },
            ServerMessage::PaneRuntimeChanged {
                pane_id,
                local,
                generation,
                agent_refs,
                state,
            } => Msg::SessionPaneRuntimeChanged {
                epoch,
                pane_id,
                local,
                generation,
                agent_refs,
                state,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconnect_does_not_treat_a_missing_or_autostarted_server_as_recovery() {
        assert!(reconnect_found_original_missing(true, false, true));
        assert!(reconnect_found_original_missing(true, true, false));
        assert!(!reconnect_found_original_missing(true, false, false));
        assert!(!reconnect_found_original_missing(false, true, true));
    }

    #[test]
    fn explicit_recreation_is_bounded_and_cancellable_without_requiring_the_original() {
        assert!(RemoteAttachMode::Recreate.reconnect());
        assert!(!RemoteAttachMode::Recreate.recover_existing());
        assert!(RemoteAttachMode::Recreate.create_only(false));
        assert!(create_only_rejects_existing(
            RemoteAttachMode::Recreate.create_only(false),
            false
        ));
        assert!(started_server_lacks_identity(true, None));
        assert!(!started_server_lacks_identity(true, Some("proof")));
        assert!(RemoteAttachMode::Recover.reconnect());
        assert!(RemoteAttachMode::Recover.recover_existing());
        assert!(!RemoteAttachMode::Recover.create_only(false));
        assert!(!RemoteAttachMode::Initial.reconnect());
    }

    #[test]
    fn reconnect_never_restarts_an_incompatible_original_session() {
        assert!(!may_restart_after_protocol_skew(true));
        assert!(may_restart_after_protocol_skew(false));
    }

    #[test]
    fn remote_attach_cancellation_is_visible_until_finished() {
        let epoch = u64::MAX - 7;
        assert!(!remote_attach_cancelled(epoch));
        cancel_remote_attach(epoch);
        assert!(remote_attach_cancelled(epoch));
        assert!(!wait_for_remote_retry(
            epoch,
            std::time::Duration::from_secs(1)
        ));
        finish_cancelled_remote_attach(epoch);
        assert!(!remote_attach_cancelled(epoch));
    }
}
