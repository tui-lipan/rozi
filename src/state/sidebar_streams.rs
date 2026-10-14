//! Stream tabs: sidebar tabs whose rows programs publish over `rozi sidebar publish`.
//!
//! A tab may have several producers at once. An extension service placed on every host runs one
//! instance per host, and each instance publishes its own part of the same tab; a session-bound
//! placement runs one per session. So rows are owned the way published activity rows are: each
//! producer sends complete snapshots of *its* rows, a new snapshot replaces only that producer's
//! rows, and its stream closing withdraws only them. Activations, expansions, and actions go back to
//! the producer that owns the row.
//!
//! A producer is identified by where it runs — the placed worker's host and session binding, or
//! this client for anything else — plus an optional `--name`, never by its process or connection.
//! A restarted producer is therefore the same producer and replaces what it published before, and
//! two hosts can both publish a row called `main` without either seeing the other's.

use std::collections::BTreeMap;

use crate::config::SidebarTabId;
use crate::state::{
    RowTarget, SidebarActionSpec, SidebarClose, SidebarRowSpec, State, TreeRow, TreeSection,
    WorkerBinding,
};

/// Who publishes a part of a stream tab.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct SidebarProducerKey {
    /// The placed worker's binding, or `None` for a process on this client.
    pub binding: Option<WorkerBinding>,
    /// Several producers of one caller in one place, told apart.
    pub name: Option<String>,
}

impl SidebarProducerKey {
    /// A stable string form: the section key rows and expansion are namespaced by.
    pub fn id(&self) -> String {
        let host = match self.binding.as_ref().map(|binding| &binding.host) {
            None | Some(crate::state::HostKey::Local) => "local".to_string(),
            Some(crate::state::HostKey::Remote(target)) => target.to_spec(),
        };
        let session = self
            .binding
            .as_ref()
            .and_then(|binding| binding.session.as_ref())
            .map(|session| format!("/{}", session.as_str()))
            .unwrap_or_default();
        let name = self
            .name
            .as_ref()
            .map(|name| format!("#{name}"))
            .unwrap_or_default();
        format!("{host}{session}{name}")
    }
}

/// One producer's live contribution to a tab.
pub struct SidebarProducer {
    pub key: SidebarProducerKey,
    /// The stream currently speaking for this producer. A replaced stream's late close carries an
    /// older id and withdraws nothing.
    pub stream_id: u64,
    /// Lines back to the producer.
    pub sender: std::sync::mpsc::SyncSender<String>,
    pub rows: Vec<SidebarRowSpec>,
    pub actions: Vec<SidebarActionSpec>,
    /// The visibility last reported to the producer, so it is told only of changes.
    pub visible: Option<bool>,
    /// The producer stopped reading: its backlog of events filled up. It is dropped, rows and all,
    /// the next time the sidebar settles.
    pub wedged: std::cell::Cell<bool>,
}

/// Every stream tab's producers, by tab and then by [`SidebarProducerKey::id`], which keeps them
/// in a stable order.
#[derive(Default)]
pub struct SidebarStreams {
    pub tabs: std::collections::HashMap<SidebarTabId, BTreeMap<String, SidebarProducer>>,
}

/// Which producer's row a stream row is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamRowRef {
    pub tab: SidebarTabId,
    pub producer: String,
    pub row: String,
}

impl State {
    /// A stream tab's sections: one per producer, headed by where it runs once there is more than
    /// one.
    pub fn stream_sections(&self, tab: &SidebarTabId) -> Vec<TreeSection> {
        let Some(producers) = self.sidebar_streams.tabs.get(tab) else {
            return Vec::new();
        };
        let mut producers: Vec<&SidebarProducer> = producers.values().collect();
        // This machine first, then hosts and sessions by name.
        producers.sort_by_key(|producer| {
            (
                producer
                    .key
                    .binding
                    .as_ref()
                    .is_some_and(|binding| binding.host != crate::state::HostKey::Local),
                self.producer_label(&producer.key),
            )
        });
        let headed = producers.len() > 1;
        producers
            .into_iter()
            .map(|producer| {
                let id = producer.key.id();
                let close_action = producer.actions.iter().find(|action| action.close);
                TreeSection {
                    key: id.clone(),
                    header: headed.then(|| self.producer_label(&producer.key)),
                    rows: producer
                        .rows
                        .iter()
                        .map(|spec| {
                            let reference = StreamRowRef {
                                tab: tab.clone(),
                                producer: id.clone(),
                                row: spec.id.clone(),
                            };
                            let close = close_action
                                .filter(|action| action_applies(spec, action))
                                .map(|action| SidebarClose::StreamAction {
                                    row: reference.clone(),
                                    action: action.id.clone(),
                                });
                            TreeRow::from_spec(spec, RowTarget::StreamRow(reference), close)
                        })
                        .collect(),
                    lazy: true,
                }
            })
            .collect()
    }

    /// How a producer's section is headed: `LOCAL` or the host, then the session it is bound to and
    /// its name when it has them.
    pub fn producer_label(&self, key: &SidebarProducerKey) -> String {
        let mut label = match key.binding.as_ref().map(|binding| &binding.host) {
            None | Some(crate::state::HostKey::Local) => "LOCAL".to_string(),
            Some(crate::state::HostKey::Remote(target)) => {
                self.remote_target_label(target).to_uppercase()
            }
        };
        if let Some(session) = key
            .binding
            .as_ref()
            .and_then(|binding| binding.session.as_ref())
            .and_then(|instance| self.session_name_of_instance(instance))
        {
            label.push_str(" · ");
            label.push_str(&session);
        }
        if let Some(name) = &key.name {
            label.push_str(" · ");
            label.push_str(name);
        }
        label
    }

    /// The name of the held session whose server is `instance`.
    fn session_name_of_instance(
        &self,
        instance: &crate::session::protocol::SessionInstanceId,
    ) -> Option<String> {
        std::iter::once(self.current())
            .chain(self.background.values())
            .find(|attachment| attachment.session_instance.as_ref() == Some(instance))
            .and_then(|attachment| attachment.session_name.clone())
    }

    /// The producer and row a stream row reference names, if both still exist.
    pub fn stream_row(&self, row: &StreamRowRef) -> Option<(&SidebarProducer, &SidebarRowSpec)> {
        let producer = self
            .sidebar_streams
            .tabs
            .get(&row.tab)?
            .get(&row.producer)?;
        let spec = producer.rows.iter().find(|spec| spec.id == row.row)?;
        Some((producer, spec))
    }
}

/// Whether `action` is one of `row`'s.
pub fn action_applies(row: &SidebarRowSpec, action: &SidebarActionSpec) -> bool {
    row.disabled.is_none()
        && row
            .actions
            .as_ref()
            .is_none_or(|ids| ids.iter().any(|id| id == &action.id))
}
