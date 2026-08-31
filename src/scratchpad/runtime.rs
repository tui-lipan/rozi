use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use tui_lipan::prelude::CommandLink;
use tui_lipan::prelude::{Context, ManagedTerminalStatus, Update};

use crate::AppRoot;
use crate::config::Config;
use crate::platform::ipc::{EndpointRegistry, IpcEndpoint};
use crate::session::client::{InboundMailbox, SessionClient};
use crate::session::protocol::Frame;

/// Mailbox epoch reserved for the client-owned scratchpad PTY host. Frames are retagged with the
/// current attachment epoch when the UI drains them, so a session switch cannot make queued
/// scratch output stale.
pub(crate) const SCRATCH_RUNTIME_EPOCH: u64 = u64::MAX;

static NEXT_RUNTIME_NONCE: AtomicU64 = AtomicU64::new(1);
const CLIENT_SCRATCH_SESSION_PREFIX: &str = "eph-client-scratch-";

pub(crate) fn is_client_scratch_session(name: &str) -> bool {
    name.starts_with(CLIENT_SCRATCH_SESSION_PREFIX)
}

/// One private PTY host for the lifetime of a Rozi UI client.
///
/// This reuses the normal session server machinery under a discovery-hidden runtime name. Only
/// this connection can address its owner-local panes, and dropping it shuts the host down.
pub(crate) struct ScratchRuntime {
    client: Option<SessionClient>,
    endpoint: Option<IpcEndpoint>,
}

impl ScratchRuntime {
    pub(crate) fn start(_config: &Config, link: CommandLink<crate::Msg>) -> io::Result<Self> {
        let nonce = NEXT_RUNTIME_NONCE.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let name = format!("{CLIENT_SCRATCH_SESSION_PREFIX}{pid}-{nonce}");
        let endpoint = EndpointRegistry::session_endpoint(&crate::control::runtime_dir()?, &name);
        endpoint.remove_stale();
        let exe = std::env::current_exe()?;
        if !exe.exists() {
            return Err(io::Error::new(
                io::ErrorKind::NotFound,
                "running rozi executable was replaced; restart rozi",
            ));
        }
        let mut child =
            crate::platform::server_lifecycle::spawn_detached_server(&exe, &name, true)?;
        let mailbox = InboundMailbox::new(SCRATCH_RUNTIME_EPOCH, name.clone(), link);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match SessionClient::connect_attached_mailbox(
                &endpoint,
                name.clone(),
                mailbox.clone(),
                false,
            ) {
                Ok((client, _)) => {
                    mailbox.activate();
                    return Ok(Self {
                        client: Some(client),
                        endpoint: Some(endpoint),
                    });
                }
                Err(error) => {
                    let exited = child.try_wait()?.is_some();
                    let retryable = matches!(
                        error.kind(),
                        io::ErrorKind::NotFound
                            | io::ErrorKind::ConnectionRefused
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::WouldBlock
                    );
                    if exited || !retryable || Instant::now() >= deadline {
                        if !exited {
                            crate::platform::server_lifecycle::terminate_server(child.id());
                        }
                        endpoint.remove_stale();
                        return Err(error);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }

    pub(crate) fn client(&self) -> Option<SessionClient> {
        self.client.clone()
    }

    pub(crate) fn shutdown(&mut self) {
        if let Some(client) = self.client.take() {
            client.shutdown();
            drop(client);
        }
        if let Some(endpoint) = self.endpoint.take() {
            endpoint.remove_stale();
        }
    }

    #[cfg(test)]
    pub(crate) fn from_test_client(client: SessionClient) -> Self {
        Self {
            client: Some(client),
            endpoint: None,
        }
    }
}

impl Drop for ScratchRuntime {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub(crate) fn message_for_frame(
    current_epoch: u64,
    frame: Frame<crate::session::protocol::ServerMessage>,
) -> Option<crate::Msg> {
    let message = crate::session::bootstrap::server_message_to_msg(current_epoch, frame);
    match &message {
        crate::Msg::SessionOutput { local: true, .. }
        | crate::Msg::SessionResized { local: true, .. }
        | crate::Msg::SessionExited { local: true, .. }
        | crate::Msg::SessionSpawnResult { local: true, .. }
        | crate::Msg::SessionPaneLoggingChanged { local: true, .. }
        | crate::Msg::SessionPaneRuntimeChanged { local: true, .. } => Some(message),
        crate::Msg::SessionError { message, .. } => {
            Some(crate::Msg::ScratchRuntimeFailed(message.clone()))
        }
        _ => None,
    }
}

pub(crate) fn failed(ctx: &mut Context<AppRoot>, message: String) -> Update {
    if ctx.state.scratch_runtime.take().is_none() {
        return Update::none();
    }
    for pane in &mut ctx.state.scratch.panes {
        pane.terminal.status = ManagedTerminalStatus::Error("scratch runtime disconnected".into());
    }
    ctx.state.scratch_visible = false;
    crate::pane::pty_events::notify_error(ctx, "Scratchpad disconnected", message);
    Update::full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::client::ClientOutbound;
    use crate::session::protocol::ClientMessage;

    #[test]
    fn scratch_frames_are_retagged_for_whichever_session_is_current() {
        let message = message_for_frame(
            41,
            Frame::PaneBytes {
                pane_id: 7,
                local: true,
                generation: 3,
                bytes: b"still-live".to_vec(),
            },
        )
        .expect("local scratch output is forwarded");
        assert!(matches!(
            message,
            crate::Msg::SessionOutput {
                epoch: 41,
                pane_id: 7,
                local: true,
                generation: 3,
                bytes,
            } if bytes == b"still-live"
        ));
    }

    #[test]
    fn client_scratch_hosts_have_a_discovery_hidden_name() {
        assert!(is_client_scratch_session("eph-client-scratch-42-1"));
        assert!(!is_client_scratch_session("eph-42"));
        assert!(!is_client_scratch_session("dev"));
    }

    #[test]
    fn dropping_the_client_runtime_requests_server_shutdown() {
        let (client, receiver) = SessionClient::test_channel();
        drop(ScratchRuntime::from_test_client(client));
        assert!(matches!(
            receiver.recv().expect("shutdown request"),
            ClientOutbound::Control(ClientMessage::Shutdown)
        ));
    }
}
