use std::collections::HashMap;

use super::AttachmentId;
use crate::config::LinkAction;
use crate::pane::id::PaneId;
use crate::session::protocol::SessionInstanceId;
use crate::session::remote::RemoteTarget;
use crate::session::remote::forward::{ForwardHandle, ForwardKey};

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
