//! Which server panes have history to compress; see [`crate::pane::idle_compression`].

use std::collections::HashMap;
use std::time::Instant;

use crate::layout::shared::ClientId;
use crate::pane::id::PaneId;
use crate::pane::idle_compression::{IDLE_BEFORE_COMPRESSION, POLL_INTERVAL, enabled};

pub(super) type PaneKey = (Option<ClientId>, PaneId);

#[derive(Debug, Default)]
pub(super) struct IdleCompression {
    /// Panes with uncompressed output, by when that output last arrived.
    waiting: HashMap<PaneKey, Instant>,
    last_poll: Option<Instant>,
}

impl IdleCompression {
    /// Output reached the pane: its history may need compressing once it goes quiet again.
    pub(super) fn note_output(&mut self, key: PaneKey, now: Instant) {
        if enabled() {
            self.waiting.insert(key, now);
        }
    }

    /// Panes quiet for long enough, at most once per [`POLL_INTERVAL`].
    pub(super) fn due(&mut self, now: Instant) -> Vec<PaneKey> {
        if self.waiting.is_empty()
            || self
                .last_poll
                .is_some_and(|last| now.duration_since(last) < POLL_INTERVAL)
        {
            return Vec::new();
        }
        self.last_poll = Some(now);
        self.waiting
            .iter()
            .filter(|(_, last)| now.duration_since(**last) >= IDLE_BEFORE_COMPRESSION)
            .map(|(key, _)| *key)
            .collect()
    }

    /// The pane has nothing left to compress, or is gone.
    pub(super) fn finished(&mut self, key: PaneKey) {
        self.waiting.remove(&key);
    }
}
