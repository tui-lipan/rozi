use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tui_lipan::prelude::*;

use crate::Msg;
use crate::events::{EventHub, EventKind};
use crate::platform::ipc::{EndpointRegistry, IpcConnection, IpcListener};
use crate::state::PaneId;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct ControlRequest {
    #[serde(flatten)]
    pub command: ControlCommand,
    #[serde(default)]
    pub source_pane: Option<PaneId>,
}

/// How many scrollback lines `capture-pane` should include when not using the visible grid.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum CaptureScrollback {
    /// Trailing line count from the retained scrollback + live grid.
    Lines(usize),
    /// Named capture modes (`"full"`, `"last-output"`).
    Named(CaptureScrollbackNamed),
}

/// Named `capture-pane` scrollback modes. Serde maps these to kebab-case strings so
/// validation lives in the type instead of string compares at each call site.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureScrollbackNamed {
    Full,
    #[serde(alias = "last_output")]
    LastOutput,
}

impl CaptureScrollback {
    pub fn parse_cli(value: &str) -> std::result::Result<Self, String> {
        if value.eq_ignore_ascii_case("full") {
            return Ok(Self::Named(CaptureScrollbackNamed::Full));
        }
        if value.eq_ignore_ascii_case("last-output") || value.eq_ignore_ascii_case("last_output") {
            return Ok(Self::Named(CaptureScrollbackNamed::LastOutput));
        }
        value
            .parse::<usize>()
            .map(Self::Lines)
            .map_err(|_| "--scrollback requires a line count or `full`".to_string())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "cmd", rename_all = "kebab-case")]
pub enum ControlCommand {
    ListPanes,
    Metrics,
    Focus {
        target: PaneId,
    },
    SendText {
        target: Option<PaneId>,
        text: String,
    },
    /// Send named keys and/or literal text chunks to a pane (tmux-style key names).
    SendKeys {
        #[serde(default)]
        target: Option<PaneId>,
        keys: Vec<String>,
        /// When true, every entry in `keys` is forwarded as literal UTF-8 (no key-name parsing).
        #[serde(default)]
        literal: bool,
    },
    NewPane {
        command: Option<String>,
        cwd: Option<String>,
        title: Option<String>,
        #[serde(default)]
        keep_open: bool,
        /// Move focus (and the active workspace) to the new pane. Defaults to `false`: the control
        /// endpoint is an automation surface, and a pane spawned by an agent must not pull the
        /// cursor out from under whoever is typing. Overrides a matched `[[rules]]` `focus`; the
        /// rule still decides workspace, float, and fullscreen.
        #[serde(default)]
        focus: bool,
    },
    /// Run any keybindable `Action` by its stable id (see `Action::id`/`Action::from_id`).
    RunAction {
        action: String,
    },
    /// Capture pane text. Without `scrollback`, returns the current visible snapshot grid.
    /// With `scrollback`, returns scrollback history (`"full"` or a trailing line count).
    CapturePane {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        scrollback: Option<CaptureScrollback>,
    },
    /// Switch the active workspace. `index` is 1-based (1-9), matching the on-screen tabs.
    SwitchWorkspace {
        index: usize,
    },
    /// Move the focused pane to another workspace. `index` is 1-based (1-9).
    MoveToWorkspace {
        index: usize,
    },
    Popup {
        command: String,
        cwd: Option<String>,
        width: Option<f32>,
        height: Option<f32>,
        title: Option<String>,
        /// Hold the popup open after the command exits, matching the `[keys]` `popup` default; set
        /// `false` for a program that owns the popup for its whole life.
        #[serde(default)]
        keep_open: Option<bool>,
    },
    /// Publish the logical agents or activities running inside the calling pane, and receive
    /// activations for them. Unlike every other command this connection stays open in both
    /// directions; closing it withdraws the pane's rows. Reached through `rozi publish`.
    Publish,
    Subscribe {
        #[serde(default)]
        events: Vec<String>,
    },
    PaneLogging {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        enabled: Option<bool>,
    },
    SetStatus {
        #[serde(default)]
        target: Option<PaneId>,
        #[serde(default)]
        status: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    },
    Pick {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        placeholder: Option<String>,
        /// Modal width in columns, clamped to a readable range. Omitted uses the shared default.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u16>,
        /// Extra chords offered beside select and cancel, advertised in the footer.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        actions: Vec<crate::state::PickAction>,
    },
}

#[derive(Clone, Debug, Serialize)]
pub struct ControlResponse {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl ControlResponse {
    pub fn ok(data: impl Serialize) -> Self {
        Self {
            ok: true,
            data: Some(serde_json::to_value(data).unwrap_or(serde_json::Value::Null)),
            error: None,
        }
    }
    pub fn empty() -> Self {
        Self {
            ok: true,
            data: None,
            error: None,
        }
    }
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            data: None,
            error: Some(message.into()),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ControlEnvelope {
    pub request: ControlRequest,
    pub reply: mpsc::Sender<ControlResponse>,
}

#[derive(Debug)]
pub struct ControlSocketGuard {
    path: PathBuf,
}

impl ControlSocketGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}
impl Drop for ControlSocketGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Runtime endpoint directory. Delegates to [`crate::platform::paths::runtime_dir`] (cross-
/// platform plan Phase 3); the private-directory creation/validation policy itself lives in
/// [`crate::platform::fs_security`].
pub fn runtime_dir() -> std::io::Result<PathBuf> {
    crate::platform::paths::runtime_dir(&crate::platform::paths::PlatformEnv::from_process())
}

#[cfg(all(test, unix))]
fn runtime_dir_with_base(base: Option<PathBuf>) -> std::io::Result<PathBuf> {
    let env = crate::platform::paths::PlatformEnv {
        xdg_runtime_dir: base.filter(|path| path.is_absolute()),
        ..Default::default()
    };
    crate::platform::paths::runtime_dir(&env)
}

pub fn socket_path_for_pid(pid: u32) -> std::io::Result<PathBuf> {
    Ok(EndpointRegistry::control_endpoint(&runtime_dir()?, pid)
        .path()
        .to_path_buf())
}

pub fn bind_control_socket() -> std::io::Result<(IpcListener, ControlSocketGuard)> {
    let path = socket_path_for_pid(std::process::id())?;
    let bound = crate::platform::ipc::IpcEndpoint::at_path(&path).bind()?;
    Ok((bound.into_listener(), ControlSocketGuard { path }))
}

pub fn run_listener(listener: IpcListener, link: CommandLink<Msg>, event_hub: EventHub) {
    listener
        .set_nonblocking(false)
        .expect("control listener supports blocking accept");
    loop {
        match listener.accept() {
            Ok(stream) => {
                let link = link.clone();
                let event_hub = event_hub.clone();
                std::thread::spawn(move || handle_connection(stream, link, event_hub));
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(err) => {
                eprintln!("rozi: control endpoint accept failed: {err}");
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}

/// How many unread activations a publisher may accumulate before rozi stops keeping them.
///
/// Activations are user clicks, so this is generous relative to how fast anyone can produce them;
/// a publisher that has stopped reading is wedged rather than busy.
const PUBLISH_ACTIVATION_BACKLOG: usize = 32;

/// Serve one pane's `publish` stream until its publisher goes away.
///
/// The only bidirectional command: after the acknowledgement, the publisher writes one row list
/// per line and rozi writes one activation per line back. Both directions run for the life of
/// the connection, so a writer thread carries activations while this thread reads.
fn run_publish_stream(mut stream: IpcConnection, link: CommandLink<Msg>, pane_id: PaneId) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };
    let _ = writeln!(
        stream,
        "{}",
        serde_json::to_string(&ControlResponse::empty()).unwrap()
    );
    // A publisher is silent between state changes, and those can be minutes apart.
    let _ = stream.set_read_timeout(None);

    let (tx, rx) = mpsc::sync_channel::<String>(PUBLISH_ACTIVATION_BACKLOG);
    link.send(Msg::PublishStreamOpen {
        pane_id,
        sender: tx,
    });
    let writer = std::thread::spawn(move || {
        while let Ok(line) = rx.recv() {
            if writer_stream.write_all(line.as_bytes()).is_err() {
                return;
            }
        }
    });

    for line in BufReader::new(reader_stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        // A malformed line is the publisher's bug, not a reason to drop its rows; skip it and keep
        // the stream open so the next good list still lands.
        if let Ok(report) = serde_json::from_str::<PublishReport>(&line) {
            link.send(Msg::PublishedRowsReported {
                pane_id,
                rows: report.rows,
            });
        }
    }

    // Reaching here means EOF or a read error: the publisher is gone, so its rows go with it.
    link.send(Msg::PublishStreamClosed { pane_id });
    drop(stream);
    let _ = writer.join();
}

/// Serve one `pick` stream until the user makes a choice, cancels, or the caller disconnects.
///
/// After acknowledging the request on the UI thread, the caller may write row updates, each
/// replacing the previous set. When the picker closes, rozi writes exactly one terminal JSON line
/// (`{"selected":"..."}` or `{"cancelled":true}`).
///
/// Note: `IpcConnection` exposes no `shutdown()`, so after writing the terminal line rozi cannot
/// force the connection closed from this end; the reader thread reaps once the client closes.
#[allow(clippy::too_many_arguments)]
fn run_pick_stream(
    mut stream: IpcConnection,
    link: CommandLink<Msg>,
    id: u64,
    title: Option<String>,
    placeholder: Option<String>,
    width: Option<u16>,
    actions: Vec<crate::state::PickAction>,
) {
    let Ok(reader_stream) = stream.try_clone() else {
        return;
    };
    let Ok(mut writer_stream) = stream.try_clone() else {
        return;
    };

    let (ack_tx, ack_rx) = mpsc::channel();
    let (reply_tx, reply_rx) = mpsc::sync_channel::<String>(1);

    link.send(Msg::PickStreamOpen {
        id,
        title,
        placeholder,
        width,
        actions,
        sender: reply_tx,
        ack: ack_tx,
    });

    let ack_response = match ack_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(res) => res,
        Err(_) => ControlResponse::error("pick request timed out"),
    };

    let _ = writeln!(stream, "{}", serde_json::to_string(&ack_response).unwrap());

    if !ack_response.ok {
        return;
    }

    let _ = stream.set_read_timeout(None);

    let writer = std::thread::spawn(move || {
        if let Ok(line) = reply_rx.recv() {
            let _ = writer_stream.write_all(line.as_bytes());
        }
    });

    for line in BufReader::new(reader_stream).lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(report) = serde_json::from_str::<PickReport>(&line) {
            link.send(Msg::PickRowsReported {
                id,
                rows: report.rows,
            });
        }
    }

    link.send(Msg::PickStreamClosed { id });
    drop(stream);
    let _ = writer.join();
}

/// One line written by a `publish` publisher.
#[derive(Debug, serde::Deserialize)]
struct PublishReport {
    #[serde(default)]
    rows: Vec<crate::session::protocol::PublishedRow>,
}

/// One line written by a `pick` publisher.
#[derive(Debug, serde::Deserialize)]
struct PickReport {
    #[serde(default)]
    rows: Vec<crate::state::PickRow>,
}

fn handle_connection(mut stream: IpcConnection, link: CommandLink<Msg>, event_hub: EventHub) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(3)));
    let reader_stream = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut line = String::new();
    if BufReader::new(reader_stream).read_line(&mut line).is_err() {
        return;
    }
    let request = match serde_json::from_str::<ControlRequest>(&line) {
        Ok(request) => request,
        Err(err) => {
            let _ = writeln!(
                stream,
                "{}",
                serde_json::to_string(&ControlResponse::error(format!("invalid request: {err}")))
                    .unwrap()
            );
            return;
        }
    };
    if let ControlCommand::Subscribe { events } = &request.command {
        let mut kinds = std::collections::HashSet::new();
        for id in events {
            let Some(kind) = EventKind::parse(id) else {
                let response = ControlResponse::error(format!("unknown event `{id}`"));
                let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
                return;
            };
            kinds.insert(kind);
        }
        let rx = event_hub.subscribe((!kinds.is_empty()).then_some(kinds));
        let _ = writeln!(
            stream,
            "{}",
            serde_json::to_string(&ControlResponse::empty()).unwrap()
        );
        let _ = stream.set_read_timeout(None);
        loop {
            match rx.recv_timeout(Duration::from_secs(30)) {
                Ok(event) => {
                    if writeln!(stream, "{event}").is_err() {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Idle liveness probe: subscribers send nothing after the request line, so
                    // EOF (or any hard error) means the peer is gone and this thread can be
                    // reaped instead of waiting for a matching event's failed write.
                    let mut probe = [0u8; 8];
                    let _ = stream.set_nonblocking(true);
                    let disconnected = match std::io::Read::read(&mut stream, &mut probe) {
                        Ok(0) => true,
                        Ok(_) => false,
                        Err(err) => err.kind() != std::io::ErrorKind::WouldBlock,
                    };
                    let _ = stream.set_nonblocking(false);
                    if disconnected {
                        return;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }
    if let ControlCommand::Publish = &request.command {
        let Some(pane_id) = request.source_pane else {
            let response = ControlResponse::error("publish requires a source pane");
            let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
            return;
        };
        run_publish_stream(stream, link, pane_id);
        return;
    }
    if let ControlCommand::Pick {
        title,
        placeholder,
        width,
        actions,
    } = &request.command
    {
        static NEXT_PICK_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = NEXT_PICK_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        run_pick_stream(
            stream,
            link,
            id,
            title.clone(),
            placeholder.clone(),
            *width,
            actions.clone(),
        );
        return;
    }
    let (tx, rx) = mpsc::channel();
    link.send(Msg::ControlRequest(ControlEnvelope { request, reply: tx }));
    let response = rx
        .recv_timeout(Duration::from_secs(10))
        .unwrap_or_else(|_| ControlResponse::error("control request timed out"));
    let _ = writeln!(stream, "{}", serde_json::to_string(&response).unwrap());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[cfg(unix)]
    fn temp_base(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rozi-test-{name}-{}", std::process::id()))
    }

    #[test]
    #[cfg(unix)]
    fn runtime_dir_uses_per_user_temp_fallback_without_xdg() {
        let expected = std::env::temp_dir().join(format!(
            "rozi-{}",
            crate::platform::fs_security::current_uid()
        ));
        assert_eq!(
            crate::platform::paths::fallback_runtime_dir_path(),
            expected
        );
    }

    #[test]
    #[cfg(unix)]
    fn runtime_dir_rejects_unsafe_existing_directory_without_chmod() {
        let base = temp_base("unsafe");
        let dir = base.join("rozi");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o777)).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);
        let mode = fs::symlink_metadata(&dir).unwrap().mode() & 0o777;
        assert_eq!(mode, 0o777);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn run_action_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::RunAction {
                action: "toggle-float".to_string(),
            },
            source_pane: Some(3),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"cmd":"run-action","action":"toggle-float","source_pane":3}"#
        );
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);
    }

    #[test]
    fn metrics_command_has_deterministic_json_shape() {
        let request = ControlRequest {
            command: ControlCommand::Metrics,
            source_pane: None,
        };
        assert_eq!(
            serde_json::to_string(&request).unwrap(),
            r#"{"cmd":"metrics","source_pane":null}"#
        );
        assert_eq!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"metrics"}"#).unwrap(),
            request
        );
    }

    #[test]
    fn capture_pane_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::CapturePane {
                target: Some(5),
                scrollback: None,
            },
            source_pane: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);

        let defaulted: ControlRequest = serde_json::from_str(r#"{"cmd":"capture-pane"}"#).unwrap();
        assert_eq!(
            defaulted.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: None
            }
        );

        let with_scrollback: ControlRequest =
            serde_json::from_str(r#"{"cmd":"capture-pane","scrollback":"full"}"#).unwrap();
        assert_eq!(
            with_scrollback.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: Some(CaptureScrollback::Named(CaptureScrollbackNamed::Full))
            }
        );

        let with_last_output: ControlRequest =
            serde_json::from_str(r#"{"cmd":"capture-pane","scrollback":"last-output"}"#).unwrap();
        assert_eq!(
            with_last_output.command,
            ControlCommand::CapturePane {
                target: None,
                scrollback: Some(CaptureScrollback::Named(CaptureScrollbackNamed::LastOutput))
            }
        );

        assert!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"capture-pane","scrollback":"100"}"#)
                .is_err()
        );

        let send_keys: ControlRequest =
            serde_json::from_str(r#"{"cmd":"send-keys","keys":["C-c","Enter"]}"#).unwrap();
        assert_eq!(
            send_keys.command,
            ControlCommand::SendKeys {
                target: None,
                keys: vec!["C-c".into(), "Enter".into()],
                literal: false,
            }
        );
    }

    #[test]
    fn switch_and_move_workspace_commands_round_trip_through_json() {
        let switch = ControlRequest {
            command: ControlCommand::SwitchWorkspace { index: 3 },
            source_pane: None,
        };
        let json = serde_json::to_string(&switch).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            switch
        );

        let move_to = ControlRequest {
            command: ControlCommand::MoveToWorkspace { index: 4 },
            source_pane: None,
        };
        let json = serde_json::to_string(&move_to).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            move_to
        );
    }

    #[test]
    fn popup_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::Popup {
                command: "fzf".into(),
                cwd: Some("/tmp".into()),
                width: Some(0.7),
                height: Some(0.5),
                title: Some("pick".into()),
                keep_open: Some(false),
            },
            source_pane: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            request
        );
    }

    #[test]
    fn set_status_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::SetStatus {
                target: Some(7),
                status: Some("blocked".into()),
                reason: Some("needs approval".into()),
            },
            source_pane: Some(3),
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            serde_json::from_str::<ControlRequest>(&json).unwrap(),
            request
        );
        assert_eq!(
            serde_json::from_str::<ControlRequest>(r#"{"cmd":"set-status"}"#)
                .unwrap()
                .command,
            ControlCommand::SetStatus {
                target: None,
                status: None,
                reason: None,
            }
        );
    }

    #[test]
    fn pick_command_round_trips_through_json() {
        let request = ControlRequest {
            command: ControlCommand::Pick {
                title: Some("Branch".into()),
                placeholder: Some("Search branches…".into()),
                width: None,
                actions: Vec::new(),
            },
            source_pane: None,
        };
        let json = serde_json::to_string(&request).unwrap();
        assert_eq!(
            json,
            r#"{"cmd":"pick","title":"Branch","placeholder":"Search branches…","source_pane":null}"#
        );
        let round_tripped: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped, request);
    }

    #[test]
    #[cfg(unix)]
    fn runtime_dir_rejects_symlink() {
        let base = temp_base("symlink");
        let dir = base.join("rozi");
        let target = temp_base("symlink-target");
        let _ = fs::remove_dir_all(&base);
        let _ = fs::remove_dir_all(&target);
        fs::create_dir_all(&base).unwrap();
        fs::create_dir_all(&target).unwrap();
        std::os::unix::fs::symlink(&target, &dir).unwrap();

        let err = runtime_dir_with_base(Some(base.clone())).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::PermissionDenied);

        let _ = fs::remove_dir_all(base);
        let _ = fs::remove_dir_all(target);
    }
}
