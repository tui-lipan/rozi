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

pub(super) fn validate_payload(
    response: &AgentResponsePayload,
) -> Result<(), (&'static str, String)> {
    if response.text.len() > MAX_RESPONSE_BYTES
        || response.turn_id.as_ref().is_some_and(|id| id.len() > 1024)
    {
        return Err((
            "invalid-argument",
            "agent response exceeds the 1 MiB text or 1024 byte turn id limit".into(),
        ));
    }
    Ok(())
}

pub(super) fn validate_published(
    publisher: &super::runtime::PanePublisher,
    rows: &[super::protocol::PublishedRow],
) -> Result<(), (&'static str, String)> {
    if publisher.responses.is_empty() {
        return Ok(());
    }
    if publisher.kind != super::protocol::PublisherKind::Agent || publisher.responses.len() > 64 {
        return Err((
            "invalid-argument",
            "only agent publishers may publish responses, at most 64 per update".into(),
        ));
    }
    let mut ids = std::collections::HashSet::new();
    for item in &publisher.responses {
        validate_payload(&item.response)?;
        if item.seq == 0 || !ids.insert(&item.id) || !rows.iter().any(|row| row.id == item.id) {
            return Err((
                "invalid-argument",
                "each response needs a unique owned row id and a positive sequence".into(),
            ));
        }
    }
    Ok(())
}

impl super::SessionServer {
    pub(super) fn store_published_responses(
        &mut self,
        owner: Option<super::ClientId>,
        pane_id: super::PaneId,
        publisher: &super::runtime::PanePublisher,
    ) {
        let Some(pane) = self.pane(owner, pane_id) else {
            return;
        };
        let runtimes = self.agent_runtimes_for(pane_id, pane);
        let accepted = publisher
            .responses
            .iter()
            .filter_map(|item| {
                let runtime =
                    runtimes.iter().find(|runtime| {
                        runtime.reference.slot.as_ref().is_some_and(|slot| {
                            slot.publisher == publisher.id && slot.id == item.id
                        })
                    })?;
                let slot = runtime.reference.clone();
                let next = (publisher.epoch, item.seq);
                if pane
                    .agent
                    .response_sequences
                    .get(&slot)
                    .is_some_and(|previous| *previous >= next)
                {
                    return None;
                }
                Some((runtime.reference.clone(), slot, next, item.response.clone()))
            })
            .collect::<Vec<_>>();
        self.pane_mut(owner, pane_id)
            .expect("pane read above")
            .agent
            .response_sequences
            .retain(|reference, _| {
                runtimes
                    .iter()
                    .any(|runtime| runtime.reference == *reference)
            });
        for (reference, slot, next, response) in accepted {
            self.pane_mut(owner, pane_id)
                .expect("pane read above")
                .agent
                .response_sequences
                .insert(slot, next);
            self.agent_responses.insert(reference, response);
        }
    }
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

    #[cfg(unix)]
    #[test]
    fn publisher_response_is_owned_sequenced_and_atomic_with_done() {
        use super::super::{SessionServer, headless::ReplyTo, protocol, runtime, tests};
        use crate::control::{AgentWaitCondition, AgentWaitResult, PublishedAgentResponse};
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(7, tests::test_pane(1));
        let (client, mut stream) = tests::attach_client(&mut server);
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(2)))
            .unwrap();
        let row = |status: &str| {
            serde_json::from_value(serde_json::json!({
                "id":"background", "title":"review", "status":status, "native_session":"native"
            }))
            .unwrap()
        };
        let publisher = |seq, response: bool| runtime::PanePublisher {
            id: "provider".into(),
            kind: protocol::PublisherKind::Agent,
            epoch: 3,
            responses: response
                .then(|| PublishedAgentResponse {
                    id: "background".into(),
                    seq,
                    response: payload("exact answer"),
                })
                .into_iter()
                .collect(),
        };
        server
            .report_pane_rows(
                client,
                7,
                1,
                false,
                publisher(0, false),
                vec![row("working")],
            )
            .unwrap();
        let reference = server.agent_runtimes_for(7, &server.panes[&7])[0]
            .reference
            .clone();
        server
            .register_post_prompt_wait(
                client,
                reference.clone(),
                AgentWaitCondition::Turn,
                None,
                None,
                true,
                ReplyTo::Attached { request_id: 42 },
            )
            .unwrap();
        server
            .report_pane_rows(client, 7, 1, false, publisher(1, true), vec![row("done")])
            .unwrap();
        assert!(server.agent_waits.is_empty());
        server.flush_clients();
        let message: protocol::ServerMessage = protocol::read_frame(&mut stream).unwrap();
        let protocol::ServerMessage::AttachedControlResult {
            request_id: 42,
            response,
        } = message
        else {
            panic!("{message:?}")
        };
        let result: AgentWaitResult = serde_json::from_value(response.data.unwrap()).unwrap();
        assert_eq!(result.response.unwrap().content.text, "exact answer");
        let fence = server.agent_responses.fence();
        server
            .report_pane_rows(client, 7, 1, false, publisher(1, true), vec![row("done")])
            .unwrap();
        assert!(server.agent_responses.after(&reference, fence).is_none());
        let mut forged = publisher(2, true);
        forged.responses[0].id = "another-publisher".into();
        assert!(
            server
                .report_pane_rows(client, 7, 1, false, forged, vec![row("done")])
                .is_err()
        );
        let mut activity = publisher(2, true);
        activity.kind = protocol::PublisherKind::Activity;
        assert!(
            server
                .report_pane_rows(client, 7, 1, false, activity, vec![row("done")])
                .is_err()
        );
        let snapshot = serde_json::to_string(&server.panes[&7].runtime).unwrap();
        assert!(!snapshot.contains("exact answer"));
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
