//! `rozi subscribe` against a session, through the real binary and a real session server process.
//!
//! No UI is attached anywhere here, which is the point: these events belong to the session.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use serde_json::Value;

const PATIENCE: Duration = Duration::from_secs(20);

/// A private set of user directories and one session in them, stopped on drop.
struct Lab {
    root: PathBuf,
    session: String,
}

impl Lab {
    fn start(name: &str) -> Self {
        // Short whatever the session is called, because the endpoint's socket path has to fit
        // SUN_LEN under a platform's own long temporary directory.
        static NEXT_LAB: AtomicU32 = AtomicU32::new(0);
        let lab = NEXT_LAB.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("rzs{}-{lab}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in ["c", "d", "s", "k", "r"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        let lab = Self {
            root,
            session: name.to_string(),
        };
        let status = lab
            .rozi(&["sessions", "new", name, "--detach"])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the session started");
        lab
    }

    fn env(&self) -> Vec<(&'static str, PathBuf)> {
        vec![
            ("XDG_CONFIG_HOME", self.root.join("c")),
            ("XDG_DATA_HOME", self.root.join("d")),
            ("XDG_STATE_HOME", self.root.join("s")),
            ("XDG_CACHE_HOME", self.root.join("k")),
            ("XDG_RUNTIME_DIR", self.root.join("r")),
            ("ROZI_CONFIG", self.root.join("c/rozi.toml")),
        ]
    }

    fn rozi(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
        command.args(args).envs(self.env()).current_dir(&self.root);
        for inherited in [
            "ROZI_PANE",
            "ROZI_SESSION",
            "ROZI_SESSION_INSTANCE",
            "ROZI_SOCKET",
            "ROZI_EXTENSION_ID",
            "ROZI_EXTENSION_GENERATION",
        ] {
            command.env_remove(inherited);
        }
        command
    }

    fn on_session(&self, args: &[&str]) -> Command {
        let mut all = vec!["--session", self.session.as_str()];
        all.extend_from_slice(args);
        self.rozi(&all)
    }

    /// Split a pane that rings the bell and exits 3, which makes every lifecycle event happen.
    fn run_scripted_pane(&self) {
        let status = self
            .on_session(&[
                "split",
                "--argv",
                "sh",
                "-c",
                "printf '\\a'; sleep 0.2; exit 3",
            ])
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the pane spawned");
    }
}

impl Drop for Lab {
    fn drop(&mut self) {
        let _ = self
            .rozi(&["sessions", "kill", &self.session])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A running subscriber whose stdout arrives a line at a time, killed on drop.
struct Stream {
    child: Child,
    lines: Receiver<String>,
}

impl Stream {
    fn open(mut command: Command) -> (Self, Option<ChildStdin>) {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (sender, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if sender.send(line).is_err() {
                    break;
                }
            }
        });
        (Self { child, lines }, stdin)
    }

    #[track_caller]
    fn next(&self) -> Value {
        let line = self
            .lines
            .recv_timeout(PATIENCE)
            .expect("the stream produced a line in time");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("{line:?} is not JSON: {err}"))
    }

    /// Every record that arrives before the stream has stayed quiet for `quiet`.
    fn until_quiet(&self, quiet: Duration) -> Vec<Value> {
        let mut records = Vec::new();
        while let Ok(line) = self.lines.recv_timeout(quiet) {
            records.push(serde_json::from_str(&line).unwrap());
        }
        records
    }

    /// Records up to and including the pane's exit.
    #[track_caller]
    fn until_pane_exits(&self) -> Vec<Value> {
        let mut records = Vec::new();
        loop {
            let record = self.next();
            let done = record["kind"] == "pane-exited";
            records.push(record);
            if done {
                return records;
            }
        }
    }

    #[track_caller]
    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the subscriber never exited");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn seqs(records: &[Value]) -> Vec<u64> {
    records
        .iter()
        .map(|record| record["seq"].as_u64().unwrap())
        .collect()
}

fn kinds(records: &[Value]) -> Vec<&str> {
    records
        .iter()
        .map(|record| record["kind"].as_str().unwrap())
        .collect()
}

#[test]
fn snapshot_tail_and_resume_agree_with_no_gap_and_no_repeat() {
    let lab = Lab::start("tail");
    let (subscriber, _stdin) = Stream::open(lab.on_session(&["subscribe", "--snapshot"]));

    let snapshot = subscriber.next();
    assert_eq!(snapshot["type"], "snapshot");
    assert_eq!(snapshot["session"], "tail");
    let instance = snapshot["session_instance"].as_str().unwrap().to_string();
    let start = snapshot["seq"].as_u64().unwrap();

    lab.run_scripted_pane();
    let tail = subscriber.until_pane_exits();

    let expected: Vec<u64> = (start + 1..start + 1 + tail.len() as u64).collect();
    assert_eq!(
        seqs(&tail),
        expected,
        "contiguous from the snapshot: {tail:?}"
    );
    assert!(tail.iter().all(|record| record["type"] == "event"));
    let order = kinds(&tail);
    assert_eq!(order[0], "pane-spawned", "announced first: {order:?}");
    assert!(order.contains(&"layout-changed"), "{order:?}");
    assert!(order.contains(&"bell"), "{order:?}");
    assert_eq!(tail.last().unwrap()["data"]["code"], "3");

    // A second subscriber resuming from the same point reads the same history.
    let cursor = format!("{instance}:{start}");
    let (resumed, _stdin) = Stream::open(lab.on_session(&["subscribe", "--after", &cursor]));
    let replay = resumed.until_pane_exits();
    assert_eq!(seqs(&replay), seqs(&tail));
    assert_eq!(kinds(&replay), kinds(&tail));

    // And a filter narrows what it reads without renumbering it.
    let (filtered, _stdin) =
        Stream::open(lab.on_session(&["subscribe", "--after", &cursor, "pane-exited"]));
    let narrowed = filtered.until_pane_exits();
    assert_eq!(kinds(&narrowed), ["pane-exited"]);
    assert_eq!(narrowed[0]["seq"], tail.last().unwrap()["seq"]);
}

/// A pane's exit is the last thing said about it. Its program goes with it, and that is reported
/// first. It takes a real process: only a real one can stop being found in the foreground.
#[test]
fn a_panes_exit_comes_after_everything_its_death_changed() {
    let lab = Lab::start("lastword");
    let (subscriber, _stdin) = Stream::open(lab.on_session(&["subscribe"]));
    let status = lab
        .on_session(&["split", "--argv", "sleep", "3"])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the pane spawned");

    let mut records = subscriber.until_pane_exits();
    let exited_at = records.len() - 1;
    records.extend(subscriber.until_quiet(Duration::from_secs(2)));

    let tail: Vec<_> = records[exited_at + 1..]
        .iter()
        .filter(|record| record["data"]["pane"] == records[exited_at]["data"]["pane"])
        .collect();
    assert!(tail.is_empty(), "said after the exit: {tail:?}");
    let program_left = records[..exited_at].iter().any(|record| {
        record["kind"] == "foreground-changed" && record["data"]["previous_program"] == "sleep"
    });
    assert!(program_left, "its program left first: {records:?}");
}

/// A title is held back until it settles, and a pane that exits straight after setting one must
/// not report it from beyond its exit.
#[test]
fn a_title_set_just_before_exiting_is_reported_before_the_exit() {
    let lab = Lab::start("lasttitle");
    let (subscriber, _stdin) = Stream::open(lab.on_session(&["subscribe"]));
    let status = lab
        .on_session(&[
            "split",
            "--argv",
            "sh",
            "-c",
            "printf '\\033]0;final\\007'; exit 0",
        ])
        .stdout(Stdio::null())
        .status()
        .unwrap();
    assert!(status.success(), "the pane spawned");

    let records = subscriber.until_pane_exits();
    assert!(
        records
            .iter()
            .any(|record| record["kind"] == "pane-title-changed"
                && record["data"]["title"] == "final"),
        "{records:?}"
    );
    let tail = subscriber.until_quiet(Duration::from_secs(1));
    assert!(tail.is_empty(), "said after the exit: {tail:?}");
}

#[test]
fn a_refused_subscription_says_why_in_a_form_a_script_can_read() {
    let lab = Lab::start("refused");
    let (first, _stdin) = Stream::open(lab.on_session(&["subscribe", "--snapshot"]));
    let snapshot = first.next();
    let instance = snapshot["session_instance"].as_str().unwrap();

    let expired = lab
        .on_session(&["subscribe", "--after", "not-this-instance:1"])
        .output()
        .unwrap();
    assert_eq!(expired.status.code(), Some(1));
    let document: Value = serde_json::from_slice(&expired.stdout).unwrap();
    assert_eq!(document["code"], "cursor-expired");
    assert_eq!(document["data"]["current_instance"], instance);

    let ui_event = lab
        .on_session(&["subscribe", "focus-changed"])
        .output()
        .unwrap();
    assert_eq!(ui_event.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&ui_event.stderr).contains("UI event"),
        "{ui_event:?}"
    );
}

/// The last record, once the subscriber has exited and its output has been read to the end.
fn last_line(stream: &Stream) -> Option<String> {
    stream.lines.try_iter().last()
}

/// Signal this lab's session server, found by the session name in its command line.
fn signal_server(lab: &Lab, signal: &str) {
    let status = Command::new("pkill")
        .args([signal, "-f", &format!("server-start {} ", lab.session)])
        .status()
        .unwrap();
    assert!(status.success(), "the session server was found");
}

#[test]
fn a_session_stopping_cleanly_ends_its_stream_with_status_zero() {
    for name in ["kill", "term"] {
        let lab = Lab::start(&format!("{name}{}", std::process::id()));
        let (mut subscriber, _stdin) = Stream::open(lab.on_session(&["subscribe", "--snapshot"]));
        assert_eq!(subscriber.next()["type"], "snapshot");

        if name == "term" {
            signal_server(&lab, "-TERM");
        } else {
            let status = lab
                .rozi(&["sessions", "kill", &lab.session])
                .stdout(Stdio::null())
                .status()
                .unwrap();
            assert!(status.success());
        }

        assert_eq!(subscriber.wait_for_exit().code(), Some(0), "{name}");
        std::thread::sleep(Duration::from_millis(50));
        let last = last_line(&subscriber).expect("a last record");
        assert_eq!(last, r#"{"type":"ended"}"#, "{name}");
    }
}

/// A supervisor restarts on failure, so a crash must not look like the session stopping.
#[test]
fn a_session_that_crashes_cuts_its_stream_off_with_status_two() {
    let lab = Lab::start(&format!("crash{}", std::process::id()));
    let (mut subscriber, _stdin) = Stream::open(lab.on_session(&["subscribe", "--snapshot"]));
    assert_eq!(subscriber.next()["type"], "snapshot");

    signal_server(&lab, "-KILL");

    assert_eq!(subscriber.wait_for_exit().code(), Some(2));
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(last_line(&subscriber), None, "no last record from a crash");
}

/// Its reader stalls, the events pile up past what the server will hold, and the subscriber then
/// reads everything it was sent, ending in `lagged`.
#[test]
fn a_subscriber_that_falls_behind_says_so_and_exits_one() {
    let lab = Lab::start("lags");
    let mut subscriber = lab
        .on_session(&["subscribe"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    // Subscribed once the first event it can hear reaches the buffer ahead of this one.
    std::thread::sleep(Duration::from_millis(300));

    // `pane-spawned` carries the command, so a long one is a large event, and a few of them outgrow
    // the server's 8 MiB backlog without loading the machine with hundreds of processes.
    let long_argument = "y".repeat(100_000);
    let mut split = vec!["split", "--argv", "true"];
    split.extend(std::iter::repeat_n(long_argument.as_str(), 5));
    for _ in 0..26 {
        let status = lab
            .on_session(&split)
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "the pane spawned");
    }

    let mut output = BufReader::new(subscriber.stdout.take().unwrap());
    let mut last = String::new();
    let mut line = String::new();
    while output.read_line(&mut line).unwrap() > 0 {
        std::mem::swap(&mut last, &mut line);
        line.clear();
    }
    let status = subscriber.wait().unwrap();

    assert_eq!(last.trim(), r#"{"type":"lagged"}"#);
    assert_eq!(status.code(), Some(1));
}

/// The far side of `rozi --remote`: exactly what ssh runs, with its stdio piped as ssh pipes it.
#[test]
fn the_remote_runner_streams_records_and_stops_when_its_input_closes() {
    use rozi::control::{ControlCommand, ControlRequest};

    let lab = Lab::start("runner");
    let (mut runner, stdin) = Stream::open(lab.rozi(&["--remote-control", "runner"]));
    let mut stdin = stdin.unwrap();
    let request = ControlRequest {
        command: ControlCommand::Subscribe {
            events: vec!["pane-spawned".to_string()],
            snapshot: false,
            after: None,
        },
        source_pane: None,
        source_session: None,
        extension: None,
        caller_context: None,
        credential: None,
    };
    writeln!(stdin, "{}", serde_json::to_string(&request).unwrap()).unwrap();
    stdin.flush().unwrap();

    assert_eq!(runner.next()["ok"], true, "the answer comes first");

    lab.run_scripted_pane();
    let event = runner.next();
    assert_eq!(event["kind"], "pane-spawned");

    // ssh going away closes this. The stream is idle, so nothing else could tell the runner.
    drop(stdin);
    assert!(runner.wait_for_exit().success());
    assert!(
        runner
            .lines
            .try_iter()
            .all(|line| line.contains("\"type\"")),
        "only records follow the answer"
    );
}
