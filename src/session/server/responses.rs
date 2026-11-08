//! Ephemeral content journal. It is deliberately absent from runtime broadcasts and snapshots.
use std::collections::{HashMap, VecDeque};

use crate::control::{AgentResponse, AgentResponsePayload};
use crate::session::protocol::AgentRef;

pub(super) const MAX_RESPONSE_BYTES: usize = 1024 * 1024;
const MAX_AGENT_BYTES: usize = 2 * MAX_RESPONSE_BYTES;
const MAX_AGENT_RESPONSES: usize = 4;
const MAX_STORE_BYTES: usize = 16 * MAX_RESPONSE_BYTES;

#[derive(Default)]
pub(super) struct AgentResponseStore {
    generation: u64,
    entries: HashMap<AgentRef, VecDeque<AgentResponse>>,
}

#[derive(Clone, Copy)]
pub(super) struct AgentResponseFence(u64);

impl AgentResponseStore {
    pub fn fence(&self) -> AgentResponseFence {
        AgentResponseFence(self.generation)
    }

    pub fn latest(&self, reference: &AgentRef) -> Option<&AgentResponse> {
        self.entries.get(reference)?.back()
    }

    pub fn after(&self, reference: &AgentRef, fence: AgentResponseFence) -> Option<AgentResponse> {
        self.entries
            .get(reference)?
            .iter()
            .find(|response| response.response_id > fence.0)
            .cloned()
    }

    pub fn insert(&mut self, reference: AgentRef, content: AgentResponsePayload) {
        if content.turn_id.is_some()
            && self.entries.get(&reference).is_some_and(|entries| {
                entries.iter().any(|entry| {
                    entry.content.turn_id == content.turn_id
                        && entry.content.source == content.source
                })
            })
        {
            return;
        }
        self.generation += 1;
        let entries = self.entries.entry(reference).or_default();
        entries.push_back(AgentResponse {
            response_id: self.generation,
            content,
        });
        while entries.len() > MAX_AGENT_RESPONSES || response_bytes(entries) > MAX_AGENT_BYTES {
            entries.pop_front();
        }
        // Bound the whole server as well: dead incarnations must not accumulate indefinitely.
        while self.entries.values().map(response_bytes).sum::<usize>() > MAX_STORE_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entries)| entries.front().map(|r| r.response_id))
                .map(|(reference, _)| reference.clone());
            let Some(reference) = oldest else { break };
            let entries = self.entries.get_mut(&reference).expect("selected above");
            entries.pop_front();
            if entries.is_empty() {
                self.entries.remove(&reference);
            }
        }
    }
}

fn response_bytes(entries: &VecDeque<AgentResponse>) -> usize {
    entries
        .iter()
        .map(|response| {
            response.content.text.len()
                + response.content.turn_id.as_ref().map_or(0, String::len)
                + 256
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{AgentResponseFidelity, AgentResponseSource};

    pub(super) fn payload(text: &str) -> AgentResponsePayload {
        AgentResponsePayload {
            text: text.into(),
            turn_id: None,
            source: AgentResponseSource::NativeHook,
            fidelity: AgentResponseFidelity::Semantic,
            complete: true,
            truncated: false,
        }
    }

    #[test]
    fn fences_and_incarnations_cannot_read_an_old_response() {
        let mut store = AgentResponseStore::default();
        let first = AgentRef::for_test(7, 1, None);
        store.insert(first.clone(), payload("old"));
        let fence = store.fence();
        assert!(store.after(&first, fence).is_none());
        store.insert(first.clone(), payload("new"));
        assert_eq!(store.after(&first, fence).unwrap().content.text, "new");
        let mut replacement = first;
        replacement.incarnation += 1;
        assert!(store.latest(&replacement).is_none());
        assert!(store.after(&replacement, fence).is_none());
    }

    #[test]
    fn retrying_a_native_turn_cannot_cross_a_prompt_fence() {
        let mut store = AgentResponseStore::default();
        let reference = AgentRef::for_test(7, 1, None);
        let mut content = payload("reply");
        content.turn_id = Some("native-turn".into());
        store.insert(reference.clone(), content.clone());
        let fence = store.fence();
        store.insert(reference.clone(), content);
        assert!(store.after(&reference, fence).is_none());
    }

    #[test]
    fn retention_bounds_count_bytes_and_dead_incarnations() {
        let mut store = AgentResponseStore::default();
        let reference = AgentRef::for_test(7, 1, None);
        for _ in 0..10 {
            store.insert(reference.clone(), payload("small"));
        }
        assert_eq!(store.entries[&reference].len(), MAX_AGENT_RESPONSES);
        let text = "🌷".repeat(MAX_RESPONSE_BYTES / 4 - 256);
        for _ in 0..4 {
            store.insert(reference.clone(), payload(&text));
        }
        assert_eq!(store.entries[&reference].len(), 2);
        assert_eq!(store.latest(&reference).unwrap().content.text, text);
        for incarnation in 2..40 {
            let mut next = reference.clone();
            next.incarnation = incarnation;
            store.insert(next, payload(&text));
        }
        assert!(store.entries.values().map(response_bytes).sum::<usize>() <= MAX_STORE_BYTES);
        assert!(store.latest(&reference).is_none());
    }
}
