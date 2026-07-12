//! End-to-end smoke coverage for the named-session wire protocol.
//!
//! The package is a binary crate, so the test launches the real `--server` entry point and speaks
//! the framed Unix-socket protocol directly. This deliberately crosses the process boundary that
//! the inline `session::server` tests stop short of covering.

use std::fs;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

const PROTOCOL_VERSION: u32 = 8;
const CONTROL_FRAME: u8 = 1;
const PANE_OUTPUT_FRAME: u8 = 2;
const PANE_ID: u32 = 41;
const PANE_GENERATION: u64 = 1;
const OUTPUT_MARKER: &[u8] = b"hyprmux-session-smoke-output";
const IO_TIMEOUT: Duration = Duration::from_secs(5);

static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(0);

struct ServerGuard {
    child: Child,
    runtime_base: PathBuf,
}

impl Drop for ServerGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = fs::remove_dir_all(&self.runtime_base);
    }
}

#[derive(Debug)]
enum WireFrame {
    Control(Value),
    PaneOutput {
        pane_id: u32,
        generation: u64,
        bytes: Vec<u8>,
    },
}

#[test]
fn real_server_replays_pane_backlog_and_layout_after_reattach() {
    let session = unique_session_name();
    let runtime_base = private_temp_dir();
    let socket = runtime_base
        .join("hyprmux")
        .join(format!("session-{session}.sock"));
    let child = Command::new(env!("CARGO_BIN_EXE_hyprmux"))
        .args(["--server", &session])
        .env("XDG_RUNTIME_DIR", &runtime_base)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("launch real session server");
    let mut server = ServerGuard {
        child,
        runtime_base,
    };

    let mut first = connect_when_ready(&socket, &mut server.child);
    write_control(&mut first, &attach_message(&session, "first"));
    let attached = expect_control_type(&mut first, "attached");
    let first_client_id = attached["client_id"].as_u64().expect("attached client id");
    assert_eq!(attached["session"], session);
    assert_eq!(attached["protocol_version"], PROTOCOL_VERSION);
    assert_eq!(attached["controller"], first_client_id);

    write_control(
        &mut first,
        &json!({
            "type": "spawn-pane",
            "pane_id": PANE_ID,
            "generation": PANE_GENERATION,
            "command": format!("printf '{}\\n'", String::from_utf8_lossy(OUTPUT_MARKER)),
            "cwd": null,
            "cols": 80,
            "rows": 24,
            "keep_open": true,
            "env": [],
            "title": "protocol smoke",
            "palette": {
                "foreground": null,
                "background": null,
                "ansi": ["Black", "Black", "Black", "Black", "Black", "Black", "Black", "Black",
                         "Black", "Black", "Black", "Black", "Black", "Black", "Black", "Black"]
            },
            "shell": ["/bin/sh"],
            "command_shell": ["/bin/sh", "-c"]
        }),
    );

    let mut saw_spawn = false;
    let mut live_output = Vec::new();
    read_until(&mut first, |frame| {
        match frame {
            WireFrame::Control(message) if message["type"] == "spawn-result" => {
                assert_eq!(message["pane_id"], PANE_ID);
                assert_eq!(message["generation"], PANE_GENERATION);
                assert_eq!(message["ok"], true, "spawn failed: {message}");
                saw_spawn = true;
            }
            WireFrame::PaneOutput {
                pane_id,
                generation,
                bytes,
            } if *pane_id == PANE_ID && *generation == PANE_GENERATION => {
                live_output.extend_from_slice(bytes);
            }
            _ => {}
        }
        saw_spawn && contains(&live_output, OUTPUT_MARKER)
    });

    let layout = json!({
        "version": 1,
        "canvas_cols": 80,
        "canvas_rows": 23,
        "workspaces": []
    });
    write_control(
        &mut first,
        &json!({"type": "commit-layout", "base_rev": 0, "layout": layout}),
    );
    let committed = expect_control_type(&mut first, "layout-committed");
    assert_eq!(committed["rev"], 1);
    assert_eq!(committed["author"], first_client_id);
    assert_eq!(committed["layout"], layout);

    write_control(&mut first, &json!({"type": "detach"}));
    drop(first);

    let mut second = UnixStream::connect(&socket).expect("reattach to live named session");
    second
        .set_read_timeout(Some(IO_TIMEOUT))
        .expect("set reattach timeout");
    second
        .set_write_timeout(Some(IO_TIMEOUT))
        .expect("set reattach timeout");
    write_control(&mut second, &attach_message(&session, "second"));
    let reattached = expect_control_type(&mut second, "attached");
    assert_eq!(reattached["layout_rev"], 1);
    assert_eq!(reattached["layout"], layout);
    assert!(
        reattached["panes"]
            .as_array()
            .expect("attached panes")
            .iter()
            .any(|pane| pane["pane_id"] == PANE_ID && pane["generation"] == PANE_GENERATION),
        "reattach omitted the live pane: {reattached}"
    );

    let mut replay = Vec::new();
    read_until(&mut second, |frame| {
        if let WireFrame::PaneOutput {
            pane_id,
            generation,
            bytes,
        } = frame
            && *pane_id == PANE_ID
            && *generation == PANE_GENERATION
        {
            replay.extend_from_slice(bytes);
        }
        contains(&replay, OUTPUT_MARKER)
    });

    write_control(&mut second, &json!({"type": "shutdown"}));
    drop(second);
    wait_for_server_exit(&mut server.child);
}

fn attach_message(session: &str, label: &str) -> Value {
    json!({
        "type": "attach",
        "session": session,
        "protocol_version": PROTOCOL_VERSION,
        "label": label,
        "read_only": false
    })
}

fn write_control(stream: &mut UnixStream, message: &Value) {
    let payload = serde_json::to_vec(message).expect("serialize client message");
    let len = u32::try_from(payload.len() + 1).expect("small smoke-test frame");
    stream
        .write_all(&len.to_be_bytes())
        .expect("write frame size");
    stream
        .write_all(&[CONTROL_FRAME])
        .expect("write frame kind");
    stream.write_all(&payload).expect("write frame payload");
    stream.flush().expect("flush client frame");
}

fn read_frame(stream: &mut UnixStream) -> io::Result<WireFrame> {
    let mut len = [0; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "empty frame"));
    }
    let mut body = vec![0; len];
    stream.read_exact(&mut body)?;
    match body[0] {
        CONTROL_FRAME => serde_json::from_slice(&body[1..])
            .map(WireFrame::Control)
            .map_err(io::Error::other),
        PANE_OUTPUT_FRAME if body.len() >= 13 => Ok(WireFrame::PaneOutput {
            pane_id: u32::from_be_bytes(body[1..5].try_into().expect("pane id header")),
            generation: u64::from_be_bytes(body[5..13].try_into().expect("generation header")),
            bytes: body[13..].to_vec(),
        }),
        kind => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unexpected server frame kind {kind}"),
        )),
    }
}

fn expect_control_type(stream: &mut UnixStream, expected: &str) -> Value {
    let mut found = None;
    read_until(stream, |frame| {
        if let WireFrame::Control(message) = frame
            && message["type"] == expected
        {
            found = Some(message.clone());
            return true;
        }
        false
    });
    found.expect("matching control frame")
}

fn read_until(stream: &mut UnixStream, mut done: impl FnMut(&WireFrame) -> bool) {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for server frame"
        );
        match read_frame(stream) {
            Ok(frame) if done(&frame) => return,
            Ok(_) => {}
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) => {}
            Err(err) => panic!("failed to read server frame: {err}"),
        }
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn connect_when_ready(socket: &Path, child: &mut Child) -> UnixStream {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        if let Ok(stream) = UnixStream::connect(socket) {
            stream
                .set_read_timeout(Some(IO_TIMEOUT))
                .expect("set client read timeout");
            stream
                .set_write_timeout(Some(IO_TIMEOUT))
                .expect("set client write timeout");
            return stream;
        }
        if let Some(status) = child.try_wait().expect("poll server process") {
            let mut stderr = String::new();
            if let Some(pipe) = child.stderr.as_mut() {
                let _ = pipe.read_to_string(&mut stderr);
            }
            panic!("session server exited early ({status}): {stderr}");
        }
        assert!(
            Instant::now() < deadline,
            "session server did not create {}",
            socket.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_server_exit(child: &mut Child) {
    let deadline = Instant::now() + IO_TIMEOUT;
    loop {
        if child.try_wait().expect("poll server shutdown").is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session server did not shut down"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn private_temp_dir() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after epoch")
        .subsec_nanos();
    let path = std::env::temp_dir().join(format!(
        "hmux-{}-{nonce:x}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&path).expect("create private runtime base");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .expect("secure private runtime base");
    path
}

fn unique_session_name() -> String {
    format!(
        "protocol-smoke-{}-{}",
        std::process::id(),
        NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed)
    )
}
