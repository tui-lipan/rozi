use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;

use tui_lipan::prelude::TerminalColorPalette;

use crate::config::UserCommandAction;

use super::PaneId;

/// A PTY-creating action deferred until an ephemeral session finishes attaching.
///
/// The launcher (and any other no-client resting state) has nowhere to run a pane. Callers stash
/// one of these, start [`crate::ops::session::attach_startup_ephemeral`], and
/// [`crate::ops::session::run_pending_session_action`] replays it from `SessionAttached`.
#[derive(Clone, Debug, PartialEq)]
pub enum PendingSessionAction {
    OpenConfigFile,
    ToggleScratchpad,
    UserCommand {
        action: UserCommandAction,
        env: Vec<(String, String)>,
    },
    NewPane {
        source: Option<PaneId>,
        command: Option<String>,
        cwd: Option<String>,
        title: Option<String>,
        keep_open: bool,
        focus: bool,
    },
    Popup {
        command: String,
        cwd: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        keep_open: bool,
    },
}

pub struct PendingSessionAttach {
    pub epoch: u64,
    pub name: String,
    pub client: Option<crate::session::client::SessionClient>,
    /// Whether a failed connect should autostart a `--server` process. Ephemeral sessions
    /// autostart; a dead named session surfaces as an error instead of a silent resurrection.
    pub autostart: bool,
    pub read_only: bool,
    /// This attach repairs an existing retained attachment rather than seeding a replacement.
    pub reconnect: bool,
    /// Remote host label when attaching via `--remote`.
    pub remote_host: Option<String>,
    pub intent: AttachIntent,
    pub left: Option<LeftSession>,
    /// The id of a session parked in the background to start this attach, restored if the attach
    /// fails. Without this, a failed remote connect would fall back to a fresh local ephemeral that
    /// re-attaches to this process's own `eph-<pid>` server — the one the parked client still
    /// controls — and join as a follower of itself. `None` when nothing was parked (a reconnect, or
    /// an attach that released rather than parked the previous session).
    pub parked_epoch: Option<super::AttachmentId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachIntent {
    Plain,
    ProfileSeed { profile: String, path: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LeftSession {
    pub name: String,
    pub was_ephemeral_shutdown: bool,
}

/// Maximum orphan bytes retained for one `(pane, generation)` key.
pub const ORPHAN_OUTPUT_CAP: usize = 256 * 1024;
/// Maximum orphan bytes retained across all panes and generations.
pub const ORPHAN_OUTPUT_GLOBAL_CAP: usize = 4 * 1024 * 1024;
/// Maximum distinct `(pane, generation)` buffers retained by one attachment.
///
/// Four thousand pending identities is already far beyond a plausible layout race, while keeping
/// tiny frames from turning the byte-bounded store into an effectively unbounded hash table.
pub const ORPHAN_OUTPUT_KEY_CAP: usize = 4 * 1024;

type OrphanOutputKey = (PaneId, u64);

/// Bounded output received before an authoritative layout creates its pane locally.
#[derive(Default)]
pub struct OrphanOutputStore {
    buffers: HashMap<OrphanOutputKey, Vec<u8>>,
    order: VecDeque<OrphanOutputKey>,
    retained: usize,
    high_water: usize,
}

/// Cheap accounting snapshot for orphan output retained by an attachment.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OrphanOutputStats {
    pub retained: usize,
    pub high_water: usize,
    pub keys: usize,
}

impl OrphanOutputStore {
    pub fn insert(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }

        let key = (pane_id, generation);
        if let Some(buffer) = self.buffers.get_mut(&key) {
            self.retained -= buffer.len();
            if bytes.len() >= ORPHAN_OUTPUT_CAP {
                buffer.clear();
                buffer.extend_from_slice(&bytes[bytes.len() - ORPHAN_OUTPUT_CAP..]);
            } else {
                buffer.extend_from_slice(bytes);
                let overflow = buffer.len().saturating_sub(ORPHAN_OUTPUT_CAP);
                if overflow > 0 {
                    buffer.drain(..overflow);
                }
            }
            self.retained += buffer.len();
        } else {
            let tail = if bytes.len() > ORPHAN_OUTPUT_CAP {
                &bytes[bytes.len() - ORPHAN_OUTPUT_CAP..]
            } else {
                bytes
            };
            self.buffers.insert(key, tail.to_vec());
            self.order.push_back(key);
            self.retained += tail.len();
        }

        while self.retained > ORPHAN_OUTPUT_GLOBAL_CAP || self.buffers.len() > ORPHAN_OUTPUT_KEY_CAP
        {
            let oldest = self.order.pop_front().expect("retained orphan has key");
            let removed = self
                .buffers
                .remove(&oldest)
                .expect("orphan order contains live key");
            self.retained -= removed.len();
        }
        self.high_water = self.high_water.max(self.retained);
        debug_assert_eq!(self.order.len(), self.buffers.len());
    }

    pub fn take(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        let key = (pane_id, generation);
        let bytes = self.buffers.remove(&key)?;
        self.retained -= bytes.len();
        self.order.retain(|candidate| *candidate != key);
        debug_assert_eq!(self.order.len(), self.buffers.len());
        Some(bytes)
    }

    /// Drop stale generations for this pane, then return only the generation in the layout.
    pub fn take_for_generation(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        let buffers = &mut self.buffers;
        let retained = &mut self.retained;
        self.order.retain(|key| {
            if key.0 == pane_id && key.1 < generation {
                let removed = buffers.remove(key).expect("orphan order contains live key");
                *retained -= removed.len();
                false
            } else {
                true
            }
        });
        self.take(pane_id, generation)
    }

    pub fn stats(&self) -> OrphanOutputStats {
        OrphanOutputStats {
            retained: self.retained,
            high_water: self.high_water,
            keys: self.buffers.len(),
        }
    }
}

/// Client-side state for an attached shared session: the layout-control lease, revision
/// bookkeeping for optimistic commits, the controller's canonical canvas, and the buffers the
/// reconciler needs. Present whenever [`super::State::session_attached`] is true under protocol v6.
pub struct SharedSessionState {
    /// This client's server-assigned id.
    pub client_id: crate::shared_layout::ClientId,
    /// The last layout revision this client has applied.
    pub layout_rev: u64,
    /// Optimistic base for the next commit: bumped locally on each commit so pipelined commits
    /// carry increasing base revs without waiting for each echo.
    pub assumed_rev: u64,
    /// The current layout controller, or `None` between promotions.
    pub controller: Option<crate::shared_layout::ClientId>,
    /// How many clients are attached to the session (including this one).
    pub clients: Vec<crate::session::protocol::ClientInfo>,
    pub input_locked: bool,
    /// Whether writable followers may immediately take the layout-control lease.
    pub allow_takeover: bool,
    pub read_only: bool,
    /// The controller's canonical pane canvas in cells (excluding the workbar). Followers letterbox
    /// to this; `None` until the first layout with a canvas is seen.
    pub canonical_canvas: Option<(u16, u16)>,
    /// The last layout this client committed/applied, used as the dirty detector for the commit
    /// chokepoint (cheaper than re-serializing).
    pub last_committed_layout: Option<crate::shared_layout::SharedLayout>,
    /// Pane output that arrived before the pane's `LayoutCommitted` created it locally, keyed by
    /// `(pane_id, generation)`; drained into the pane once the reconciler adds it.
    orphan_output: OrphanOutputStore,
    /// Latest pending resize per `(local, pane)` while the controller debounces resize storms.
    pub pending_resizes: HashMap<(bool, PaneId), (u16, u16)>,
    /// Whether a trailing-edge `Msg::FlushPaneResizes` is already in flight, so a burst of resizes
    /// schedules only one flush timer.
    pub resize_flush_scheduled: bool,
    /// Whether a trailing-edge `Msg::FlushLayoutCommit` is already in flight.
    pub layout_commit_scheduled: bool,
}

impl SharedSessionState {
    pub fn new(client_id: crate::shared_layout::ClientId) -> Self {
        Self {
            client_id,
            layout_rev: 0,
            assumed_rev: 0,
            controller: None,
            clients: Vec::new(),
            input_locked: false,
            allow_takeover: false,
            read_only: false,
            canonical_canvas: None,
            last_committed_layout: None,
            orphan_output: OrphanOutputStore::default(),
            pending_resizes: HashMap::new(),
            resize_flush_scheduled: false,
            layout_commit_scheduled: false,
        }
    }

    /// True when this client currently holds the layout-control lease.
    pub fn is_controller(&self) -> bool {
        self.controller == Some(self.client_id)
    }

    /// How many attached clients are actually using the session, ignoring the ones parked in the
    /// background. This is the count that decides whether the session is shared in any sense the
    /// user should be told about.
    pub fn active_clients(&self) -> usize {
        self.clients.iter().filter(|client| !client.parked).count()
    }

    /// Whether any other client has an outstanding request for the control lease (badge fodder for
    /// the controller's workbar and the session-clients view).
    pub fn has_pending_control_requests(&self) -> bool {
        self.clients
            .iter()
            .any(|client| client.requesting_control && Some(client.id) != self.controller)
    }

    /// Buffer pane output that arrived before its pane exists locally, enforcing the per-pane cap
    /// by dropping the oldest bytes.
    pub fn buffer_orphan_output(&mut self, pane_id: PaneId, generation: u64, bytes: &[u8]) {
        self.orphan_output.insert(pane_id, generation, bytes);
    }

    pub fn take_orphan_output(&mut self, pane_id: PaneId, generation: u64) -> Option<Vec<u8>> {
        self.orphan_output.take_for_generation(pane_id, generation)
    }

    pub fn orphan_output_stats(&self) -> OrphanOutputStats {
        self.orphan_output.stats()
    }
}

/// A pane spawn deferred until a session client is available (see [`super::State::pending_spawns`]).
#[derive(Clone, Debug)]
pub struct PendingPaneSpawn {
    pub pane_id: PaneId,
    pub local: bool,
    pub generation: u64,
    pub command: Option<String>,
    pub cwd: Option<String>,
    pub cols: u16,
    pub rows: u16,
    pub keep_open: bool,
    pub env: Vec<(String, String)>,
    pub title: Option<String>,
    pub palette: TerminalColorPalette,
    pub shell: Vec<String>,
    pub command_shell: Vec<String>,
}

/// The prefix that marks an auto-named ephemeral session. Ephemeral servers shut down on a clean
/// quit but survive a UI crash for reattach; user-typed names may not use this prefix.
pub const EPHEMERAL_SESSION_PREFIX: &str = "eph-";

/// Whether `name` denotes an auto-managed ephemeral session.
pub fn is_ephemeral_session_name(name: &str) -> bool {
    name.starts_with(EPHEMERAL_SESSION_PREFIX)
}

/// The ephemeral session name for this UI process (`eph-<pid>`).
pub fn ephemeral_session_name() -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}", std::process::id())
}

/// A fresh ephemeral name that will not collide with a still-running ephemeral server left behind
/// by a prior detach (`eph-<pid>-<salt>`).
pub fn fresh_ephemeral_session_name(salt: u64) -> String {
    format!("{EPHEMERAL_SESSION_PREFIX}{}-{salt}", std::process::id())
}

/// Ephemeral name qualified by a stable per-client identifier (`eph-<host>-<pid>`), for `--remote`.
///
/// A bare `eph-<pid>` names a session that lives on the *remote* host, where two clients on
/// different machines can plausibly share a pid and would silently land on the same session. The
/// hostname disambiguates them; it stays constant for the process lifetime, so a dropped link
/// reconnects to the same ephemeral name.
pub fn remote_ephemeral_session_name() -> String {
    let host = crate::platform::user::hostname()
        .map(|host| {
            // Session names permit only `[A-Za-z0-9_-]`; keep the alphanumerics and cap the length
            // so an odd or very long hostname cannot produce an invalid or unwieldy name.
            host.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(24)
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| "host".to_string());
    format!("{EPHEMERAL_SESSION_PREFIX}{host}-{}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orphan_store_keeps_the_newest_per_key_tail_and_ignores_empty_input() {
        let mut store = OrphanOutputStore::default();
        store.insert(1, 7, &[]);
        assert_eq!(store.stats(), OrphanOutputStats::default());

        let first = vec![1; ORPHAN_OUTPUT_CAP - 2];
        store.insert(1, 7, &first);
        store.insert(1, 7, &[2, 3, 4, 5]);

        let retained = store.take(1, 7).expect("buffered generation");
        assert_eq!(retained.len(), ORPHAN_OUTPUT_CAP);
        assert_eq!(&retained[..2], &[1, 1]);
        assert_eq!(&retained[ORPHAN_OUTPUT_CAP - 4..], &[2, 3, 4, 5]);
        assert_eq!(store.stats().retained, 0);
        assert_eq!(store.stats().keys, 0);
        assert_eq!(store.order.len(), 0);
    }

    #[test]
    fn orphan_store_evicts_oldest_whole_buffers_at_global_budget() {
        let mut store = OrphanOutputStore::default();
        for pane_id in 0..=16 {
            store.insert(pane_id, 1, &vec![pane_id as u8; ORPHAN_OUTPUT_CAP]);
        }

        assert!(!store.buffers.contains_key(&(0, 1)));
        assert!(store.buffers.contains_key(&(1, 1)));
        assert!(store.buffers.contains_key(&(16, 1)));
        assert_eq!(
            store.stats(),
            OrphanOutputStats {
                retained: ORPHAN_OUTPUT_GLOBAL_CAP,
                high_water: ORPHAN_OUTPUT_GLOBAL_CAP,
                keys: 16,
            }
        );
        assert_eq!(store.order.len(), store.buffers.len());
    }

    #[test]
    fn orphan_store_evicts_oldest_whole_keys_during_a_tiny_frame_flood() {
        let mut store = OrphanOutputStore::default();
        for pane_id in 0..ORPHAN_OUTPUT_KEY_CAP as PaneId + 3 {
            store.insert(pane_id, 1, &[pane_id as u8]);
        }

        assert_eq!(store.stats().keys, ORPHAN_OUTPUT_KEY_CAP);
        assert_eq!(store.stats().retained, ORPHAN_OUTPUT_KEY_CAP);
        assert_eq!(store.stats().high_water, ORPHAN_OUTPUT_KEY_CAP);
        assert!(!store.buffers.contains_key(&(0, 1)));
        assert!(!store.buffers.contains_key(&(1, 1)));
        assert!(!store.buffers.contains_key(&(2, 1)));
        assert_eq!(store.order.front(), Some(&(3, 1)));
        assert_eq!(
            store.order.back(),
            Some(&(ORPHAN_OUTPUT_KEY_CAP as PaneId + 2, 1))
        );
        assert_eq!(store.order.len(), store.buffers.len());
    }

    #[test]
    fn orphan_store_take_updates_accounting_and_order() {
        let mut store = OrphanOutputStore::default();
        store.insert(1, 1, b"abc");
        store.insert(2, 1, b"defgh");

        assert_eq!(store.take(1, 1), Some(b"abc".to_vec()));
        assert_eq!(store.stats().retained, 5);
        assert_eq!(store.stats().high_water, 8);
        assert_eq!(store.stats().keys, 1);
        assert_eq!(store.order.iter().copied().collect::<Vec<_>>(), [(2, 1)]);
        assert_eq!(store.take(1, 1), None);
    }

    #[test]
    fn orphan_store_discards_only_superseded_generations() {
        let mut store = OrphanOutputStore::default();
        store.insert(4, 2, b"old");
        store.insert(4, 3, b"exact");
        store.insert(4, 4, b"future");
        store.insert(5, 1, b"other");

        assert_eq!(store.take_for_generation(4, 3), Some(b"exact".to_vec()));
        assert!(!store.buffers.contains_key(&(4, 2)));
        assert_eq!(
            store.buffers.get(&(4, 4)).map(Vec::as_slice),
            Some(&b"future"[..])
        );
        assert_eq!(
            store.buffers.get(&(5, 1)).map(Vec::as_slice),
            Some(&b"other"[..])
        );
        assert_eq!(
            store.stats(),
            OrphanOutputStats {
                retained: b"future".len() + b"other".len(),
                high_water: b"old".len() + b"exact".len() + b"future".len() + b"other".len(),
                keys: 2,
            }
        );
        assert_eq!(
            store.order.iter().copied().collect::<Vec<_>>(),
            [(4, 4), (5, 1)]
        );
    }
}
