use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};

use serde::Serialize;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum EventKind {
    PaneSpawned,
    PaneExited,
    PaneStatusChanged,
    FocusChanged,
    WorkspaceSwitched,
    SessionAttached,
    SessionDetached,
    SessionRenamed,
    SessionCreated,
    ControllerChanged,
    ClientJoined,
    ClientLeft,
    ProfileLoaded,
    ProfileApplied,
    ProfileSaved,
    ConfigReloaded,
}

impl EventKind {
    pub const ALL: [Self; 16] = [
        Self::PaneSpawned,
        Self::PaneExited,
        Self::PaneStatusChanged,
        Self::FocusChanged,
        Self::WorkspaceSwitched,
        Self::SessionAttached,
        Self::SessionDetached,
        Self::SessionRenamed,
        Self::SessionCreated,
        Self::ControllerChanged,
        Self::ClientJoined,
        Self::ClientLeft,
        Self::ProfileLoaded,
        Self::ProfileApplied,
        Self::ProfileSaved,
        Self::ConfigReloaded,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::PaneSpawned => "pane-spawned",
            Self::PaneExited => "pane-exited",
            Self::PaneStatusChanged => "pane-status-changed",
            Self::FocusChanged => "focus-changed",
            Self::WorkspaceSwitched => "workspace-switched",
            Self::SessionAttached => "session-attached",
            Self::SessionDetached => "session-detached",
            Self::SessionRenamed => "session-renamed",
            Self::SessionCreated => "session-created",
            Self::ControllerChanged => "controller-changed",
            Self::ClientJoined => "client-joined",
            Self::ClientLeft => "client-left",
            Self::ProfileLoaded => "profile-loaded",
            Self::ProfileApplied => "profile-applied",
            Self::ProfileSaved => "profile-saved",
            Self::ConfigReloaded => "config-reloaded",
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
    run_hooks(state, &event);
}

/// Publish an event to this client's subscribers, but run its hooks only on the current layout
/// controller. Server-owned transitions reach every client, so this prevents duplicate external
/// side effects while preserving each client's local `subscribe` stream.
pub fn emit_with_controller_hooks(state: &crate::state::State, event: Event) {
    state.event_hub.publish(&event);
    if state.is_controller() {
        run_hooks(state, &event);
    }
}

fn run_hooks(state: &crate::state::State, event: &Event) {
    let commands: Vec<_> = state
        .config
        .hooks
        .iter()
        .filter(|hook| hook.event == event.kind)
        .map(|hook| hook.run.clone())
        .collect();
    if commands.is_empty() {
        return;
    }
    let env = hook_env_for_state(event, state);
    let runner = crate::platform::command::resolve_command_shell(
        state.config.command_shell.as_deref(),
        &crate::platform::command::ShellEnv::from_process(),
    );
    for command in commands {
        let env = env.clone();
        let runner = runner.clone();
        std::thread::spawn(move || {
            let _ = std::process::Command::new(runner.program)
                .args(runner.args)
                .arg(command)
                .envs(env)
                .spawn();
        });
    }
}

fn hook_env(event: &Event, control_socket_path: Option<&std::path::Path>) -> Vec<(String, String)> {
    let mut env = vec![("HYPRMUX_EVENT".to_string(), event.kind.id().to_string())];
    if let Some(path) = control_socket_path {
        env.push(("HYPRMUX_SOCKET".to_string(), path.display().to_string()));
    }
    env.extend(event.fields.iter().map(|(key, value)| {
        (
            format!("HYPRMUX_{}", key.to_ascii_uppercase()),
            value.clone(),
        )
    }));
    env
}

/// Build hook environment, including the remote host when the client is `--remote` attached.
pub(crate) fn hook_env_for_state(
    event: &Event,
    state: &crate::state::State,
) -> Vec<(String, String)> {
    let mut env = hook_env(event, state.control_socket_path.as_deref());
    if let Some(host) = &state.current().remote_host {
        env.push(("HYPRMUX_REMOTE_HOST".to_string(), host.clone()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_kind_ids_round_trip() {
        assert_eq!(EventKind::ALL.len(), 16);
        let mut ids = HashSet::new();
        for kind in EventKind::ALL {
            assert_eq!(EventKind::parse(kind.id()), Some(kind));
            assert!(ids.insert(kind.id()));
        }
        assert_eq!(EventKind::parse("not-an-event"), None);
    }

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
        let env = hook_env(
            &Event::new(
                EventKind::WorkspaceSwitched,
                vec![("workspace", "2".into())],
            ),
            Some(std::path::Path::new("/tmp/hyprmux.sock")),
        );
        assert!(env.contains(&("HYPRMUX_EVENT".into(), "workspace-switched".into())));
        assert!(env.contains(&("HYPRMUX_WORKSPACE".into(), "2".into())));
        assert!(env.contains(&("HYPRMUX_SOCKET".into(), "/tmp/hyprmux.sock".into())));
    }

    #[cfg(unix)]
    #[test]
    fn emit_fans_out_all_matching_hooks() {
        use crate::config::{HyprmuxConfig, HyprmuxHookConfig};
        use std::time::{Duration, Instant};

        let dir = std::env::temp_dir().join(format!("hyprmux-hook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let second = dir.join("second");
        let socket = dir.join("control.sock");
        let config = HyprmuxConfig {
            hooks: vec![
                HyprmuxHookConfig {
                    event: EventKind::PaneExited,
                    run: format!("printf '%s' \"$HYPRMUX_SOCKET\" > '{}'", first.display()),
                },
                HyprmuxHookConfig {
                    event: EventKind::PaneExited,
                    run: format!("printf '%s' \"$HYPRMUX_SOCKET\" > '{}'", second.display()),
                },
                HyprmuxHookConfig {
                    event: EventKind::PaneSpawned,
                    run: format!("touch '{}'", dir.join("wrong-event").display()),
                },
            ],
            ..HyprmuxConfig::default()
        };
        let mut state = crate::state::State::new(config, tui_lipan::prelude::Theme::default());
        state.control_socket_path = Some(socket.clone());

        emit(&state, Event::new(EventKind::PaneExited, vec![]));

        let deadline = Instant::now() + Duration::from_secs(2);
        while (!first.exists() || !second.exists()) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            std::fs::read_to_string(first).unwrap(),
            socket.display().to_string()
        );
        assert_eq!(
            std::fs::read_to_string(second).unwrap(),
            socket.display().to_string()
        );
        assert!(!dir.join("wrong-event").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
