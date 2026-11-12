use std::collections::HashMap;

use super::AttachmentId;
use tui_lipan::prelude::TextInput;
use crate::config::LinkAction;
use crate::pane::id::PaneId;
use crate::session::protocol::SessionInstanceId;
use crate::session::remote::RemoteTarget;
use crate::session::remote::forward::{ForwardHandle, ForwardHost, ForwardKey};

/// Lifecycle state of a client-owned SSH port forward.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ForwardState {
    Starting,
    Ready,
    Reconnecting,
    AuthRequired,
    Failed,
}

/// How an activated terminal URL is opened once any required local tunnel is ready.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenDestination {
    System,
    Handler(LinkAction),
}

/// Originating attachment context pinned when a link activation waits on a tunnel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingLinkContext {
    pub epoch: AttachmentId,
    pub target: RemoteTarget,
    pub source_pane: PaneId,
    pub source_session: Option<SessionInstanceId>,
    pub source_session_name: Option<String>,
    pub remote_host: Option<String>,
}

/// Client-side destination resolved at link activation time so async tunnel readiness never
/// re-evaluates placement, scratchpad visibility, or session identity against newer state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolvedLinkDestination {
    System,
    Exec {
        command: String,
        env: Vec<(String, String)>,
    },
    ExecDirect {
        argv: Vec<String>,
        env: Vec<(String, String)>,
    },
    ScratchRun {
        command: String,
        keep_open: bool,
        env: Vec<(String, String)>,
    },
}

/// A link activation waiting for its tunnel to become ready.
#[derive(Clone, Debug)]
pub struct PendingLinkOpen {
    pub original_url: String,
    pub context: PendingLinkContext,
    pub destination: ResolvedLinkDestination,
}

/// One active or recently failed client-owned port forward.
pub struct PortForwardEntry {
    pub key: ForwardKey,
    pub local_port: u16,
    /// True once this forward has emitted its first `Ready`, locking `local_port` for the life of
    /// the entry (including across `AuthRequired`/`Failed` restarts).
    pub port_pinned: bool,
    pub state: ForwardState,
    pub generation: u64,
    pub handle: Option<ForwardHandle>,
    pub error: Option<String>,
    pub pending_opens: Vec<PendingLinkOpen>,
}

/// Client-global registry of SSH port forwards, independent of session attachments.
#[derive(Default)]
pub struct PortForwardRuntimeState {
    forwards: HashMap<ForwardKey, PortForwardEntry>,
    next_generation: u64,
}

impl PortForwardRuntimeState {
    pub fn get(&self, key: &ForwardKey) -> Option<&PortForwardEntry> {
        self.forwards.get(key)
    }

    pub fn get_mut(&mut self, key: &ForwardKey) -> Option<&mut PortForwardEntry> {
        self.forwards.get_mut(key)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&ForwardKey, &PortForwardEntry)> {
        self.forwards.iter()
    }

    pub fn is_empty(&self) -> bool {
        self.forwards.is_empty()
    }

    pub fn len(&self) -> usize {
        self.forwards.len()
    }

    pub fn mint_generation(&mut self) -> u64 {
        self.next_generation = self.next_generation.wrapping_add(1).max(1);
        self.next_generation
    }

    pub fn insert(&mut self, entry: PortForwardEntry) -> Option<PortForwardEntry> {
        self.forwards.insert(entry.key.clone(), entry)
    }

    pub fn remove(&mut self, key: &ForwardKey) -> Option<PortForwardEntry> {
        self.forwards.remove(key)
    }

    pub fn remove_target(&mut self, target: &RemoteTarget) -> Vec<PortForwardEntry> {
        let keys: Vec<_> = self
            .forwards
            .keys()
            .filter(|key| &key.target == target)
            .cloned()
            .collect();
        keys.into_iter()
            .filter_map(|key| self.forwards.remove(&key))
            .collect()
    }

    pub fn clear(&mut self) {
        self.forwards.clear();
    }

    pub fn matches_generation(&self, key: &ForwardKey, generation: u64) -> bool {
        self.forwards
            .get(key)
            .is_some_and(|entry| entry.generation == generation)
    }
}

/// Editable fields in the manual *Add port forward* modal form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortFormField {
    RemoteHost,
    RemotePort,
    LocalPort,
}

impl PortFormField {
    pub const ORDER: [Self; 3] = [Self::RemoteHost, Self::RemotePort, Self::LocalPort];

    pub fn label(self) -> &'static str {
        match self {
            Self::RemoteHost => "Remote host",
            Self::RemotePort => "Remote port",
            Self::LocalPort => "Local port",
        }
    }

    pub fn placeholder(self) -> &'static str {
        match self {
            Self::RemoteHost => "localhost",
            Self::RemotePort => "5173",
            Self::LocalPort => "same as remote",
        }
    }
}

/// State of the manual *Add forward* three-line modal dialog.
#[derive(Clone, Debug)]
pub struct PortFormState {
    pub target: RemoteTarget,
    pub remote_host: TextInput,
    pub remote_port: TextInput,
    pub local_port: TextInput,
    pub focus: PortFormField,
    pub error: Option<String>,
}

impl PortFormState {
    pub fn new(target: RemoteTarget) -> Self {
        Self {
            target,
            remote_host: TextInput::new("localhost"),
            remote_port: TextInput::new(""),
            local_port: TextInput::new(""),
            focus: PortFormField::RemotePort,
            error: None,
        }
    }

    pub fn input(&self, field: PortFormField) -> &TextInput {
        match field {
            PortFormField::RemoteHost => &self.remote_host,
            PortFormField::RemotePort => &self.remote_port,
            PortFormField::LocalPort => &self.local_port,
        }
    }

    pub fn input_mut(&mut self, field: PortFormField) -> &mut TextInput {
        match field {
            PortFormField::RemoteHost => &mut self.remote_host,
            PortFormField::RemotePort => &mut self.remote_port,
            PortFormField::LocalPort => &mut self.local_port,
        }
    }

    pub fn cycle_focus(&mut self, forward: bool) {
        let len = PortFormField::ORDER.len();
        let index = PortFormField::ORDER
            .iter()
            .position(|field| *field == self.focus)
            .unwrap_or(0);
        let step = if forward { 1 } else { len - 1 };
        self.focus = PortFormField::ORDER[(index + step) % len];
    }

    /// Validate form inputs into a [`ForwardKey`] and optional explicit local port (`0` means auto).
    pub fn parse_request(&self) -> Result<(ForwardKey, u16), String> {
        let host_text = self.remote_host.text().trim();
        let remote_host = if host_text.is_empty() {
            ForwardHost::Localhost
        } else {
            ForwardHost::parse(host_text)?
        };
        let remote_port = parse_nonzero_port(self.remote_port.text(), "remote port")?;
        let local_port = if self.local_port.text().trim().is_empty() {
            0
        } else {
            parse_nonzero_port(self.local_port.text(), "local port")?
        };
        Ok((
            ForwardKey {
                target: self.target.clone(),
                remote_host,
                remote_port,
            },
            local_port,
        ))
    }
}

fn parse_nonzero_port(raw: &str, label: &str) -> Result<u16, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(format!("enter a {label} (1–65535)"));
    }
    trimmed
        .parse::<u16>()
        .ok()
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("invalid {label} `{trimmed}`"))
}

/// Overlay state for the active `Ports…` modal.
#[derive(Clone, Debug)]
pub struct PortsOverlayState {
    pub query: TextInput,
    pub selected: Option<ForwardKey>,
    pub pending_close: Option<ForwardKey>,
    pub form: Option<PortFormState>,
}

impl PortsOverlayState {
    pub fn new(selected: Option<ForwardKey>) -> Self {
        Self {
            query: TextInput::new(""),
            selected,
            pending_close: None,
            form: None,
        }
    }
}
