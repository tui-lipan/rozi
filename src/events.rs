use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum EventKind {
    PaneSpawned,
    PaneExited,
    FocusChanged,
    WorkspaceSwitched,
}

impl EventKind {
    pub const ALL: [Self; 4] = [
        Self::PaneSpawned,
        Self::PaneExited,
        Self::FocusChanged,
        Self::WorkspaceSwitched,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::PaneSpawned => "pane-spawned",
            Self::PaneExited => "pane-exited",
            Self::FocusChanged => "focus-changed",
            Self::WorkspaceSwitched => "workspace-switched",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.id() == value)
    }
}

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: EventKind,
    pub fields: Vec<(&'static str, String)>,
}

impl Event {
    pub fn new(kind: EventKind, fields: Vec<(&'static str, String)>) -> Self {
        Self { kind, fields }
    }

    fn json(&self) -> String {
        #[derive(Serialize)]
        struct WireEvent<'a> {
            event: &'static str,
            data: HashMap<&'static str, &'a str>,
        }
        let data = self
            .fields
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .collect();
        serde_json::to_string(&WireEvent {
            event: self.kind.id(),
            data,
        })
        .expect("event fields serialize")
    }
}

struct Subscriber {
    tx: mpsc::SyncSender<String>,
    kinds: Option<HashSet<EventKind>>,
}

#[derive(Clone, Default)]
pub struct EventHub(Arc<Mutex<Vec<Subscriber>>>);

impl EventHub {
    pub fn subscribe(&self, kinds: Option<HashSet<EventKind>>) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::sync_channel(128);
        self.0.lock().unwrap().push(Subscriber { tx, kinds });
        rx
    }

    pub fn publish(&self, event: &Event) {
        let mut subscribers = self.0.lock().unwrap();
        // Zero subscribers is the common case (hover-focus emits on every pane crossing);
        // skip the JSON serialization entirely then.
        if subscribers.is_empty() {
            return;
        }
        let json = event.json();
        subscribers.retain(|subscriber| {
            if subscriber
                .kinds
                .as_ref()
                .is_some_and(|kinds| !kinds.contains(&event.kind))
            {
                return true;
            }
            subscriber.tx.try_send(json.clone()).is_ok()
        });
    }
}

pub fn emit(state: &crate::state::State, event: Event) {
    state.event_hub.publish(&event);
    if let Some(command) = state.config.hooks.get(event.kind.id()).cloned() {
        let env = hook_env(&event);
        let runner = crate::platform::command::resolve_command_shell(
            state.config.command_shell.as_deref(),
            &crate::platform::command::ShellEnv::from_process(),
        );
        std::thread::spawn(move || {
            let _ = std::process::Command::new(runner.program)
                .args(runner.args)
                .arg(command)
                .envs(env)
                .spawn();
        });
    }
}

fn hook_env(event: &Event) -> Vec<(String, String)> {
    let mut env = vec![("HYPRMUX_EVENT".to_string(), event.kind.id().to_string())];
    env.extend(event.fields.iter().map(|(key, value)| {
        (
            format!("HYPRMUX_{}", key.to_ascii_uppercase()),
            value.clone(),
        )
    }));
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_json_is_stable() {
        let event = Event::new(
            EventKind::PaneExited,
            vec![("pane", "3".into()), ("code", "7".into())],
        );
        let value: serde_json::Value = serde_json::from_str(&event.json()).unwrap();
        assert_eq!(
            value,
            serde_json::json!({"event":"pane-exited","data":{"pane":"3","code":"7"}})
        );
    }

    #[test]
    fn hub_filters_and_drops_full_subscribers() {
        let hub = EventHub::default();
        let rx = hub.subscribe(Some(HashSet::from([EventKind::PaneExited])));
        hub.publish(&Event::new(EventKind::PaneSpawned, vec![]));
        assert!(rx.try_recv().is_err());
        hub.publish(&Event::new(EventKind::PaneExited, vec![]));
        assert!(rx.try_recv().is_ok());

        let stalled = hub.subscribe(None);
        for _ in 0..129 {
            hub.publish(&Event::new(EventKind::FocusChanged, vec![]));
        }
        drop(stalled);
        hub.publish(&Event::new(EventKind::FocusChanged, vec![]));
    }

    #[test]
    fn hook_environment_uses_public_names() {
        let env = hook_env(&Event::new(
            EventKind::WorkspaceSwitched,
            vec![("workspace", "2".into())],
        ));
        assert!(env.contains(&("HYPRMUX_EVENT".into(), "workspace-switched".into())));
        assert!(env.contains(&("HYPRMUX_WORKSPACE".into(), "2".into())));
    }
}
