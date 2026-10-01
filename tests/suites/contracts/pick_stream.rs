//! `rozi pick --json` against a live control socket: the stream contract a producer sees.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use rozi::{AppRoot, Msg};
use tui_lipan::TestBackend;

/// Generous on purpose: the `rozi pick` under test is a freshly linked debug executable, and its
/// first start on a loaded Windows runner, past a virus scanner that has never seen it, can take
/// most of ten seconds on its own. That says nothing about the stream contract this file checks.
const BUDGET: Duration = Duration::from_secs(30);

/// Pump the app until `predicate` holds. With a `child`, fail as soon as it exits instead of
/// waiting out the budget, and say why: a client that could not connect exits early, and a bare
/// timeout would hide that behind an empty picker.
fn pump_until(
    backend: &mut TestBackend<AppRoot>,
    mut child: Option<&mut Child>,
    mut predicate: impl FnMut(&TestBackend<AppRoot>) -> bool,
) {
    let deadline = Instant::now() + BUDGET;
    while !predicate(backend) {
        backend.render();
        let _ = backend.pump();
        if let Some(child) = child.as_deref_mut()
            && let Some(status) = child.try_wait().unwrap()
        {
            panic!(
                "`rozi pick` exited with {status} before the picker got there; stderr: {}",
                stderr_of(child)
            );
        }
        if Instant::now() >= deadline {
            let stderr = child.map(|child| {
                let _ = child.kill();
                stderr_of(child)
            });
            panic!(
                "timed out with pages {:?}; `rozi pick` stderr: {stderr:?}",
                backend.state().pick.as_ref().map(|pick| &pick.pages)
            );
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn stderr_of(child: &mut Child) -> String {
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    stderr
}

/// A tabbed picker reports every event on one stream, in order: an action that keeps the picker
/// open, the tab switch, and the terminal selection, each carrying the tab it happened on.
///
/// The picker used to forward only its first reply line, so anything after an open-ended action
/// was silently lost and the caller waited forever on a picker that had already closed.
#[test]
fn a_tabbed_picker_reports_actions_tab_switches_and_the_selection_in_order() {
    rozi::test_support::isolate_user_dirs();
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            let mut backend = TestBackend::new(rozi::test_support::configured_app());
            pump_until(&mut backend, None, |backend| {
                backend.state().control_socket_path.is_some()
            });
            let socket = backend.state().control_socket_path.clone().unwrap();

            let mut child = Command::new(env!("CARGO_BIN_EXE_rozi"))
                .args(["pick", "--json"])
                .env("ROZI_SOCKET", &socket)
                .env_remove("ROZI_PANE")
                .env_remove("ROZI_EXTENSION")
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            let mut stdin = child.stdin.take().unwrap();
            writeln!(
                stdin,
                "{}",
                serde_json::json!({
                    "title": "Git",
                    "tabs": [{ "id": "branches", "label": "Branches" }, { "id": "tags" }],
                    "actions": [{ "id": "refresh", "key": "ctrl-r", "label": "refresh" }],
                    "rows": [{ "id": "main", "label": "main" }],
                })
            )
            .unwrap();
            // Filled while hidden, which is how a producer preloads a cheap tab.
            writeln!(
                stdin,
                "{}",
                serde_json::json!({ "tab": "tags", "rows": [{ "id": "v1", "label": "v1.0" }] })
            )
            .unwrap();
            stdin.flush().unwrap();

            pump_until(&mut backend, Some(&mut child), |backend| {
                backend.state().pick.as_ref().is_some_and(|pick| {
                    pick.pages.len() == 2 && pick.pages.iter().all(|page| !page.rows.is_empty())
                })
            });
            let pick = backend.state().pick.as_ref().unwrap();
            assert_eq!(pick.pages[0].rows[0].id.as_deref(), Some("main"));
            assert_eq!(pick.pages[1].label, "tags", "the label defaults to the id");

            backend.dispatch(Msg::PickActionKey(0)).unwrap();
            backend.dispatch(Msg::PickTabSelected(1)).unwrap();
            backend.dispatch(Msg::PickActivate(0)).unwrap();

            let deadline = Instant::now() + BUDGET;
            while child.try_wait().unwrap().is_none() {
                backend.render();
                let _ = backend.pump();
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    panic!("pick never exited; stderr: {}", stderr_of(&mut child));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            drop(stdin);
            let status = child.wait().unwrap();
            assert!(
                status.success(),
                "a selection exits 0, got {status}; stderr: {}",
                stderr_of(&mut child)
            );

            let lines: Vec<serde_json::Value> = BufReader::new(child.stdout.take().unwrap())
                .lines()
                .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
                .collect();
            assert_eq!(
                lines,
                vec![
                    serde_json::json!({ "action": "refresh", "selected": "main", "tab": "branches" }),
                    serde_json::json!({ "tab": "tags" }),
                    serde_json::json!({ "selected": "v1", "tab": "tags" }),
                ]
            );
            let _ = backend.dispatch(Msg::RunAction(rozi::input::Action::Quit));
        })
        .unwrap()
        .join()
        .unwrap();
}
