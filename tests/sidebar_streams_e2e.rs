//! A real `rozi sidebar publish` process feeding a stream tab of a real UI, through the same CLI
//! bridge and control listener a user's script uses.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rozi::config::{SidebarTab, SidebarTabId};
use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;

fn pump_until(
    backend: &mut TestBackend<AppRoot>,
    timeout: Duration,
    mut predicate: impl FnMut(&TestBackend<AppRoot>) -> bool,
) {
    let deadline = Instant::now() + timeout;
    while !predicate(backend) {
        backend.render();
        let _ = backend.pump();
        assert!(Instant::now() < deadline, "condition timed out");
        std::thread::sleep(Duration::from_millis(10));
    }
}

struct CleanupBackend(TestBackend<AppRoot>);

impl Drop for CleanupBackend {
    fn drop(&mut self) {
        let _ = self.0.dispatch(Msg::RunAction(rozi::input::Action::Quit));
    }
}

#[test]
fn a_published_stream_fills_its_tab_and_hears_activations() {
    rozi::test_support::isolate_user_dirs();
    rozi::test_support::on_large_stack(|| {
        std::fs::write(
            rozi::config::config_path(),
            "[sidebar]\ntabs = [{ name = \"deploys\", label = \"Deploys\", stream = true }]\n",
        )
        .unwrap();
        let id = SidebarTabId::new("deploys");
        let mut guard = CleanupBackend(TestBackend::new(rozi::test_support::configured_app()));
        let backend = &mut guard.0;
        assert!(
            backend
                .state()
                .config
                .sidebar
                .tabs
                .iter()
                .any(|tab| matches!(tab, SidebarTab::Stream { .. })),
            "the config declares a stream tab"
        );
        {
            let state = backend.state_mut();
            state.sidebar.shown = [true, false];
            state.sidebar.panels[0].tabs = vec![id.clone()];
            state.sidebar.panels[0].active_tab = Some(id.clone());
        }
        pump_until(backend, Duration::from_secs(5), |backend| {
            backend.state().control_socket_path.is_some()
        });
        let socket = backend.state().control_socket_path.clone().unwrap();

        let mut child = Command::new(env!("CARGO_BIN_EXE_rozi"))
            .args(["sidebar", "publish", "--tab", "deploys"])
            .env("ROZI_SOCKET", &socket)
            // The developer's own rozi identity must not leak into this isolated app.
            .env_remove("ROZI_EXTENSION")
            .env_remove("ROZI_EXTENSION_CREDENTIAL")
            .env_remove("ROZI_PANE")
            .env_remove("ROZI_SESSION_INSTANCE")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let (lines, events) = std::sync::mpsc::channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if lines.send(line).is_err() {
                    return;
                }
            }
        });
        writeln!(
                stdin,
                r#"{{"rows":[{{"id":"api","label":"api","tone":"working"}},{{"id":"eu","parent":"api","label":"eu-west-1"}}]}}"#
            )
            .unwrap();
        stdin.flush().unwrap();
        pump_until(backend, Duration::from_secs(10), |backend| {
            backend
                .state()
                .sidebar_streams
                .tabs
                .get(&id)
                .is_some_and(|producers| {
                    producers.values().any(|producer| producer.rows.len() == 2)
                })
        });
        let next_event = |backend: &mut TestBackend<AppRoot>| {
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if let Ok(line) = events.try_recv() {
                    return line;
                }
                backend.render();
                let _ = backend.pump();
                assert!(Instant::now() < deadline, "no event reached the producer");
                std::thread::sleep(Duration::from_millis(10));
            }
        };
        assert_eq!(next_event(backend), r#"{"visible":true}"#);

        let tab = SidebarTab::Stream {
            name: id.clone(),
            label: "Deploys".into(),
        };
        let index = backend
            .state()
            .tree_items(&tab)
            .unwrap()
            .iter()
            .position(|item| item.line().is_some_and(|line| line.row.id == "eu"))
            .expect("the child row is listed under its parent");
        backend
            .dispatch(Msg::SidebarRowActivate { panel: 0, index })
            .unwrap();
        assert_eq!(next_event(backend), r#"{"activate":"eu"}"#);

        // The producer exits: its rows go with it.
        drop(stdin);
        child.kill().ok();
        child.wait().unwrap();
        pump_until(backend, Duration::from_secs(10), |backend| {
            !backend.state().sidebar_streams.tabs.contains_key(&id)
        });
    });
}
