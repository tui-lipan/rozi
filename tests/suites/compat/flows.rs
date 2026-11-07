//! Session flows that both binaries can run once each advertises generation 52 or newer.
//!
//! The controller is one binary and the server is the other. A second client of the server's
//! binary reads the same panes, which is the follower. `subscribe` runs on the controller so the
//! event stream crosses the version boundary too.

use std::io::{BufRead, BufReader, Read};
use std::process::Child;
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use super::harness::{self, Mode, Session, Transport, pair};

const PATIENCE: Duration = Duration::from_secs(20);
const MARKER: &str = "compat-marker";
const ECHO: &str = "while IFS= read -r line; do printf \"%s\\n\" \"$line\"; done";

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release"]
fn mixed_versions_drive_a_session() {
    if !harness::skip_unless_enabled() {
        return;
    }
    let pair = pair();
    if pair.mode() != Mode::Window {
        eprintln!(
            "skipping: full flows wait until both binaries are generation >= {} (released {}, built {})",
            harness::SUPPORT_WINDOW_GENERATION,
            pair.old_protocol,
            pair.new_protocol
        );
        return;
    }
    exercise(&pair.old_bin, &pair.new_bin, Transport::Local);
    exercise(&pair.new_bin, &pair.old_bin, Transport::Local);
}

pub(crate) fn exercise(
    server: &std::path::Path,
    controller: &std::path::Path,
    transport: Transport,
) {
    let session = Session::start(server);
    #[cfg(unix)]
    if transport == Transport::Ssh {
        session.env.pin_localhost_binary(server);
    }
    let live = Live::subscribe(controller, &session);
    live.until("type", "snapshot");

    let split = session.call(
        controller,
        transport,
        &["split", "--argv", "sh", "-c", ECHO],
    );
    assert!(
        split.success(),
        "split did not open a pane:\n{}",
        split.combined()
    );
    live.until("kind", "pane-spawned");

    let panes = harness::pane_ids(&session.list_panes(server));
    assert_eq!(
        panes.len(),
        1,
        "the session should hold the pane that was just opened"
    );
    let id = panes[0].to_string();

    let typed = session.call(
        controller,
        transport,
        &[
            "send-keys",
            "--target",
            &id,
            "--wait-for",
            MARKER,
            "--timeout",
            "15s",
            "--",
            MARKER,
            "Enter",
        ],
    );
    assert!(
        typed.success(),
        "send-keys did not land in the pane:\n{}",
        typed.combined()
    );

    let followed = session.client(
        server,
        &[
            "capture-pane",
            "--target",
            &id,
            "--wait-for",
            MARKER,
            "--timeout",
            "15s",
        ],
    );
    assert!(
        followed.success() && followed.combined().contains(MARKER),
        "follower did not see the controller's input:\n{}",
        followed.combined()
    );

    let reported = session.call(
        controller,
        transport,
        &[
            "agents",
            "report",
            "--target",
            &id,
            "--agent",
            "compat-agent",
            "--integration",
            "compat",
            "--state",
            "working",
            "--seq",
            "1",
        ],
    );
    assert!(
        reported.success(),
        "agents report failed:\n{}",
        reported.combined()
    );
    let agents = session.client(server, &["agents", "list", "--format", "json"]);
    assert!(
        agents.success() && agents.combined().contains("compat-agent"),
        "follower did not see the reported agent:\n{}",
        agents.combined()
    );

    let closed = session.call(controller, transport, &["pane", "close", "--target", &id]);
    assert!(
        closed.success(),
        "pane close failed:\n{}",
        closed.combined()
    );
    live.until("kind", "pane-exited");
}

/// `rozi subscribe` for one session. Drop kills it.
struct Live {
    child: Child,
    lines: Receiver<std::io::Result<String>>,
    stderr: Arc<Mutex<String>>,
}

impl Live {
    fn subscribe(bin: &std::path::Path, session: &Session) -> Self {
        let mut child = harness::spawn_reader(
            bin,
            &session.env,
            &["--session", &session.name, "subscribe", "--snapshot"],
        );
        let stdout = child.stdout.take().expect("subscriber stdout");
        let stderr = child.stderr.take().expect("subscriber stderr");
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr_text = Arc::new(Mutex::new(String::new()));
        let slot = Arc::clone(&stderr_text);
        std::thread::spawn(move || {
            let mut text = String::new();
            let _ = BufReader::new(stderr).read_to_string(&mut text);
            if let Ok(mut slot) = slot.lock() {
                *slot = text;
            }
        });
        Self {
            child,
            lines,
            stderr: stderr_text,
        }
    }

    fn until(&self, field: &str, value: &str) -> Value {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                panic!(
                    "subscriber did not emit {field}={value} within {}s\n{}",
                    PATIENCE.as_secs(),
                    self.stderr
                        .lock()
                        .map(|text| text.clone())
                        .unwrap_or_default()
                );
            }
            let line = self
                .lines
                .recv_timeout(remaining)
                .unwrap_or_else(|_| {
                    panic!(
                        "subscriber closed before {field}={value}\n{}",
                        self.stderr
                            .lock()
                            .map(|text| text.clone())
                            .unwrap_or_default()
                    )
                })
                .unwrap_or_else(|error| panic!("subscriber read failed: {error}"));
            let Ok(parsed) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            if parsed.get(field).and_then(Value::as_str) == Some(value) {
                return parsed;
            }
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
