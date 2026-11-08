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
    entries: HashMap<AgentRef, VecDeque<StoredResponse>>,
}

struct StoredResponse {
    response: AgentResponse,
    native_session: Option<String>,
}

#[derive(Clone)]
pub(super) struct AgentResponseFence {
    generation: u64,
    pub(super) native_session: Option<String>,
}

impl AgentResponseStore {
    pub fn fence(&self, native_session: Option<String>) -> AgentResponseFence {
        AgentResponseFence {
            generation: self.generation,
            native_session,
        }
    }

    pub fn latest(
        &self,
        reference: &AgentRef,
        native_session: &Option<String>,
    ) -> Option<&AgentResponse> {
        self.entries
            .get(reference)?
            .iter()
            .rev()
            .find(|entry| &entry.native_session == native_session)
            .map(|entry| &entry.response)
    }

    pub fn after(
        &self,
        reference: &AgentRef,
        fence: &AgentResponseFence,
    ) -> Option<&AgentResponse> {
        self.entries
            .get(reference)?
            .iter()
            .find(|entry| {
                entry.response.response_id > fence.generation
                    && (fence.native_session.is_none()
                        || entry.native_session == fence.native_session)
            })
            .map(|entry| &entry.response)
    }

    pub fn insert(
        &mut self,
        reference: AgentRef,
        native_session: Option<String>,
        content: AgentResponsePayload,
    ) {
        if self.contains_turn(&reference, &native_session, &content) {
            return;
        }
        self.generation += 1;
        let entries = self.entries.entry(reference).or_default();
        entries.push_back(StoredResponse {
            response: AgentResponse {
                response_id: self.generation,
                content,
            },
            native_session,
        });
        while entries.len() > MAX_AGENT_RESPONSES || response_bytes(entries) > MAX_AGENT_BYTES {
            entries.pop_front();
        }
        self.evict_oldest();
    }
    fn contains_turn(
        &self,
        reference: &AgentRef,
        native_session: &Option<String>,
        content: &AgentResponsePayload,
    ) -> bool {
        content.turn_id.is_some()
            && self.entries.get(reference).is_some_and(|entries| {
                entries.iter().any(|entry| {
                    &entry.native_session == native_session
                        && entry.response.content.turn_id == content.turn_id
                        && entry.response.content.source == content.source
                })
            })
    }

    fn evict_oldest(&mut self) {
        // Bound the whole server as well: dead incarnations must not accumulate indefinitely.
        while self.entries.values().map(response_bytes).sum::<usize>() > MAX_STORE_BYTES {
            let oldest = self
                .entries
                .iter()
                .min_by_key(|(_, entries)| entries.front().map(|r| r.response.response_id))
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

fn response_bytes(entries: &VecDeque<StoredResponse>) -> usize {
    entries
        .iter()
        .map(|response| {
            response.response.content.text.len()
                + response.native_session.as_ref().map_or(0, String::len)
                + response
                    .response
                    .content
                    .turn_id
                    .as_ref()
                    .map_or(0, String::len)
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
    // Ownership is checked against the rows that will actually be installed: sanitizing cleans
    // and truncates ids and drops rows past the cap, and a response naming anything else would
    // pass here and then find no row to belong to.
    let canonical = super::runtime::canonical_row_ids(rows);
    let mut ids = std::collections::HashSet::new();
    for item in &publisher.responses {
        validate_payload(&item.response)?;
        if item.seq == 0 || !ids.insert(&item.id) || !canonical.contains(&item.id) {
            return Err((
                "invalid-argument",
                "each response needs a unique owned row id and a positive sequence".into(),
            ));
        }
    }
    Ok(())
}

impl super::SessionServer {
    pub(super) fn response_native_session(&self, reference: &AgentRef) -> Option<String> {
        let pane = self.panes.get(&reference.pane.pane_id)?;
        super::protocol::occupant_native_session(
            &pane.runtime.rows,
            pane.runtime.integration.as_deref(),
            reference.slot.as_ref(),
        )
    }

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
                Some((
                    runtime.reference.clone(),
                    slot,
                    next,
                    super::protocol::occupant_native_session(
                        &pane.runtime.rows,
                        pane.runtime.integration.as_deref(),
                        runtime.reference.slot.as_ref(),
                    ),
                    item.response.clone(),
                ))
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
        for (reference, slot, next, native_session, response) in accepted {
            self.pane_mut(owner, pane_id)
                .expect("pane read above")
                .agent
                .response_sequences
                .insert(slot, next);
            self.agent_responses
                .insert(reference, native_session, response);
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
        store.insert(first.clone(), None, payload("old"));
        let fence = store.fence(None);
        assert!(store.after(&first, &fence).is_none());
        store.insert(first.clone(), None, payload("new"));
        assert_eq!(store.after(&first, &fence).unwrap().content.text, "new");
        let mut replacement = first;
        replacement.incarnation += 1;
        assert!(store.latest(&replacement, &None).is_none());
        assert!(store.after(&replacement, &fence).is_none());
    }

    #[test]
    fn native_session_fences_separate_conversations_in_one_process() {
        let mut store = AgentResponseStore::default();
        let reference = AgentRef::for_test(7, 1, None);
        let fence = store.fence(Some("first-session".into()));
        store.insert(
            reference.clone(),
            Some("second-session".into()),
            payload("other conversation"),
        );
        assert!(store.after(&reference, &fence).is_none());
        assert!(
            store
                .latest(&reference, &Some("first-session".into()))
                .is_none()
        );
        assert_eq!(
            store
                .latest(&reference, &Some("second-session".into()))
                .unwrap()
                .content
                .text,
            "other conversation"
        );
    }

    #[test]
    fn retrying_a_native_turn_cannot_cross_a_prompt_fence() {
        let mut store = AgentResponseStore::default();
        let reference = AgentRef::for_test(7, 1, None);
        let mut content = payload("reply");
        content.turn_id = Some("native-turn".into());
        store.insert(reference.clone(), None, content.clone());
        let fence = store.fence(None);
        store.insert(reference.clone(), None, content);
        assert!(store.after(&reference, &fence).is_none());
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
        let fence = server.agent_responses.fence(Some("native".into()));
        server
            .report_pane_rows(client, 7, 1, false, publisher(1, true), vec![row("done")])
            .unwrap();
        assert!(server.agent_responses.after(&reference, &fence).is_none());
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
    fn a_response_must_name_a_row_that_survives_sanitizing() {
        use super::super::{SessionServer, protocol, runtime, tests};
        use crate::control::PublishedAgentResponse;
        let mut server = SessionServer::new_named("dev");
        server.panes.insert(7, tests::test_pane(1));
        let (client, _stream) = tests::attach_client(&mut server);
        let long = "x".repeat(100);
        let row = |id: &str| -> protocol::PublishedRow {
            serde_json::from_value(serde_json::json!({
                "id": id, "title": "t", "status": "done", "native_session": "native"
            }))
            .unwrap()
        };
        let publisher = |id: &str| runtime::PanePublisher {
            id: "provider".into(),
            kind: protocol::PublisherKind::Agent,
            epoch: 1,
            responses: vec![PublishedAgentResponse {
                id: id.into(),
                seq: 1,
                response: payload("answer"),
            }],
        };
        let truncated =
            server.report_pane_rows(client, 7, 1, false, publisher(&long), vec![row(&long)]);
        assert!(
            truncated.is_err(),
            "an id that sanitizing would change cannot own a response"
        );
        let mut rows: Vec<_> = (0..70).map(|n| row(&format!("row-{n}"))).collect();
        let past_cap =
            server.report_pane_rows(client, 7, 1, false, publisher("row-69"), rows.clone());
        assert!(
            past_cap.is_err(),
            "a row past the cap cannot own a response"
        );
        rows.truncate(3);
        server
            .report_pane_rows(client, 7, 1, false, publisher("row-2"), rows)
            .unwrap();
    }

    #[test]
    fn retention_bounds_count_bytes_and_dead_incarnations() {
        let mut store = AgentResponseStore::default();
        let reference = AgentRef::for_test(7, 1, None);
        for _ in 0..10 {
            store.insert(reference.clone(), None, payload("small"));
        }
        assert_eq!(store.entries[&reference].len(), MAX_AGENT_RESPONSES);
        let text = "🌷".repeat(MAX_RESPONSE_BYTES / 4 - 256);
        for _ in 0..4 {
            store.insert(reference.clone(), None, payload(&text));
        }
        assert_eq!(store.entries[&reference].len(), 2);
        assert_eq!(store.latest(&reference, &None).unwrap().content.text, text);
        for incarnation in 2..40 {
            let mut next = reference.clone();
            next.incarnation = incarnation;
            store.insert(next, None, payload(&text));
        }
        assert!(store.entries.values().map(response_bytes).sum::<usize>() <= MAX_STORE_BYTES);
        assert!(store.latest(&reference, &None).is_none());
    }
}
