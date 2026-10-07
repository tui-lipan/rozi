use std::collections::HashSet;
use std::sync::{Arc, Mutex, mpsc};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub enum EventKind {
    PaneSpawned,
    PaneExited,
    PaneStatusChanged,
    Bell,
    FocusChanged,
    WorkspaceSwitched,
    LayoutChanged,
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
    pub const ALL: [Self; 18] = [
        Self::PaneSpawned,
        Self::PaneExited,
        Self::PaneStatusChanged,
        Self::Bell,
        Self::FocusChanged,
        Self::WorkspaceSwitched,
        Self::LayoutChanged,
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
            Self::Bell => "bell",
            Self::FocusChanged => "focus-changed",
            Self::WorkspaceSwitched => "workspace-switched",
            Self::LayoutChanged => "layout-changed",
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

/// One `subscribe` event as it goes out on the wire.
///
/// `data` is deliberately a flat map of strings rather than a per-kind payload type. The fields an
/// event carries grow over time and a subscriber is expected to read the ones it knows and ignore
/// the rest, so the schema describes the envelope and the closed vocabulary of event names without
/// freezing each kind's field list.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[cfg_attr(feature = "schema-gen", derive(schemars::JsonSchema))]
pub struct WireEvent {
    pub event: EventKind,
    pub data: std::collections::BTreeMap<String, String>,
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
        serde_json::to_string(&WireEvent {
            event: self.kind,
            data: self
                .fields
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        })
        .expect("event fields serialize")
    }
}

struct Subscriber {
    tx: mpsc::SyncSender<String>,
    kinds: Option<HashSet<EventKind>>,
    binding: Option<crate::state::WorkerBinding>,
}

#[derive(Clone, Default)]
pub struct EventHub(Arc<Mutex<Vec<Subscriber>>>);

/// The session on screen when an event happened, which is what a placed worker's subscription is
/// filtered by.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventScope {
    pub host: crate::state::HostKey,
    pub session: Option<crate::session::protocol::SessionInstanceId>,
}

impl EventScope {
    /// The scope an event belongs to: the session on screen, unless the event is about one of this
    /// client's own panes (the scratchpad, the popup), which belong to this machine whatever session
    /// is showing. An id the local namespace shares with a session pane counts as local, so
    /// ambiguity never reaches a worker on another host.
    pub fn of(state: &crate::state::State, event: &Event) -> Self {
        let local_pane = event
            .fields
            .iter()
            .find(|(key, _)| *key == "pane")
            .and_then(|(_, value)| value.parse().ok())
            .is_some_and(|id| crate::pane::lifecycle::pane_is_local(state, id));
        if local_pane {
            return Self {
                host: crate::state::HostKey::Local,
                session: None,
            };
        }
        Self {
            host: crate::state::HostKey::of(state.current().remote_target.as_ref()),
            session: state.current().session_instance.clone(),
        }
    }
}

impl EventHub {
    pub fn subscribe(&self, kinds: Option<HashSet<EventKind>>) -> mpsc::Receiver<String> {
        self.subscribe_bound(kinds, None)
    }

    /// Subscribe on behalf of a placed worker, which hears an event only when it happened while
    /// the session on screen lay within `binding`.
    pub fn subscribe_bound(
        &self,
        kinds: Option<HashSet<EventKind>>,
        binding: Option<crate::state::WorkerBinding>,
    ) -> mpsc::Receiver<String> {
        let (tx, rx) = mpsc::sync_channel(128);
        self.0
            .lock()
            .unwrap()
            .push(Subscriber { tx, kinds, binding });
        rx
    }

    /// Publish an event that belongs to no session. Placed workers do not hear it.
    pub fn publish(&self, event: &Event) {
        self.publish_scoped(event, None);
    }

    pub fn publish_scoped(&self, event: &Event, scope: Option<&EventScope>) {
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
            if let Some(binding) = &subscriber.binding
                && !scope.is_some_and(|scope| binding.admits(&scope.host, scope.session.as_ref()))
            {
                return true;
            }
            subscriber.tx.try_send(json.clone()).is_ok()
        });
    }
}

pub fn emit(state: &crate::state::State, event: Event) {
    state
        .event_hub
        .publish_scoped(&event, Some(&EventScope::of(state, &event)));
    run_hooks(state, &event);
}

/// Publish an event to this client's subscribers, but run its hooks only on the current layout
/// controller. Server-owned transitions reach every client, so this prevents duplicate external
/// side effects while preserving each client's local `subscribe` stream.
pub fn emit_with_controller_hooks(state: &crate::state::State, event: Event) {
    state
        .event_hub
        .publish_scoped(&event, Some(&EventScope::of(state, &event)));
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
        let _ = crate::jobs::try_spawn(move || {
            if let Ok(mut child) = std::process::Command::new(runner.program)
                .args(runner.args)
                .arg(command)
                .envs(env)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                let _ = child.wait();
            }
        });
    }
}

fn hook_env(event: &Event, control_socket_path: Option<&std::path::Path>) -> Vec<(String, String)> {
    let mut env = vec![("ROZI_EVENT".to_string(), event.kind.id().to_string())];
    if let Some(path) = control_socket_path {
        env.push(("ROZI_SOCKET".to_string(), path.display().to_string()));
    }
    // Hooks always run on the client, so this client's own path is the right one even when the
    // session it is watching lives on another host.
    if let Some(path) = crate::platform::paths::current_binary() {
        env.push(("ROZI_BIN".to_string(), path.display().to_string()));
    }
    env.extend(
        event
            .fields
            .iter()
            .map(|(key, value)| (format!("ROZI_{}", key.to_ascii_uppercase()), value.clone())),
    );
    env
}

/// Build hook environment, including the remote host when the client is `--remote` attached.
pub(crate) fn hook_env_for_state(
    event: &Event,
    state: &crate::state::State,
) -> Vec<(String, String)> {
    let mut env = hook_env(event, state.control_socket_path.as_deref());
    if let Some(host) = &state.current().remote_host {
        env.push(("ROZI_REMOTE_HOST".to_string(), host.clone()));
    }
    env
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A placed worker hears what happens in a session within its binding, and nothing that
    /// happened while another host's session - or no session - was on screen.
    #[test]
    fn a_bound_subscriber_hears_only_events_from_sessions_within_its_binding() {
        let hub = EventHub::default();
        let pc = crate::state::HostKey::Remote(crate::session::remote::RemoteTarget::Alias(
            "pc".to_string(),
        ));
        let bound = hub.subscribe_bound(
            None,
            Some(crate::state::WorkerBinding {
                host: pc.clone(),
                session: None,
            }),
        );
        let everything = hub.subscribe(None);
        let local = EventScope {
            host: crate::state::HostKey::Local,
            session: None,
        };
        let remote = EventScope {
            host: pc,
            session: None,
        };
        hub.publish_scoped(&Event::new(EventKind::PaneSpawned, vec![]), Some(&local));
        hub.publish(&Event::new(EventKind::ConfigReloaded, vec![]));
        hub.publish_scoped(&Event::new(EventKind::PaneExited, vec![]), Some(&remote));
        let heard: Vec<_> = bound.try_iter().collect();
        assert_eq!(heard.len(), 1);
        assert!(heard[0].contains("pane-exited"), "{heard:?}");
        assert_eq!(everything.try_iter().count(), 3);
    }

    #[test]
    fn event_kind_ids_round_trip() {
        assert_eq!(EventKind::ALL.len(), 18);
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
            Some(std::path::Path::new("/tmp/rozi.sock")),
        );
        assert!(env.contains(&("ROZI_EVENT".into(), "workspace-switched".into())));
        assert!(env.contains(&("ROZI_WORKSPACE".into(), "2".into())));
        assert!(env.contains(&("ROZI_SOCKET".into(), "/tmp/rozi.sock".into())));
        // A hook that calls back into rozi should not have to assume a `PATH` install either.
        assert!(
            env.iter().any(|(key, value)| key == "ROZI_BIN"
                && std::path::Path::new(value) == std::env::current_exe().unwrap()),
            "hooks learn the binary path: {env:?}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn emit_fans_out_all_matching_hooks() {
        use crate::config::{Config, HookConfig};
        use std::time::{Duration, Instant};

        let _jobs = crate::jobs::lock_for_tests();

        let dir = std::env::temp_dir().join(format!("rozi-hook-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first");
        let second = dir.join("second");
        let socket = dir.join("control.sock");
        let config = Config {
            hooks: vec![
                HookConfig {
                    event: EventKind::PaneExited,
                    run: format!("printf '%s' \"$ROZI_SOCKET\" > '{}'", first.display()),
                },
                HookConfig {
                    event: EventKind::PaneExited,
                    run: format!("printf '%s' \"$ROZI_SOCKET\" > '{}'", second.display()),
                },
                HookConfig {
                    event: EventKind::PaneSpawned,
                    run: format!("touch '{}'", dir.join("wrong-event").display()),
                },
            ],
            ..Config::default()
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
