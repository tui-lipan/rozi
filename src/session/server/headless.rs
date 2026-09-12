//! Headless control: running a [`ControlCommand`] against a session server with no UI attached.
//!
//! The UI control endpoint (`src/control.rs`) answers the same commands from a client's `State`.
//! This is the other half: the session server answers them from the state it already owns — the
//! PTYs, the server-side `TerminalScreen` per pane, the runtime state agent detection writes, and
//! the shared layout document. Nothing here needs a client, which is the point. A detached `dev`
//! can be inspected, captured, typed into, and grown a pane from a shell script or an SSH login
//! that never starts a terminal UI.
//!
//! Two rules keep this honest:
//!
//! - **No new authority.** Every command here is one an attached client could already send. A
//!   headless caller reaches the endpoint through the same private, per-user runtime directory a
//!   client does, and gets nothing an attached client would not.
//! - **No UI pretending.** Focus, workspace switching, popups, toasts, pickers, and actions are
//!   client-local by design (see `architecture.md`'s runtime invariants). A session server has no
//!   answer for them, so they are refused by name rather than silently accepted or faked.

use serde::Serialize;

use super::*;
use crate::control::{
    CaptureScrollback, CaptureScrollbackNamed, ControlCommand, ControlRequest, ControlResponse,
};
use crate::layout::shared::{
    FracRect, SHARED_LAYOUT_VERSION, SharedLayout, SharedPane, SharedWorkspace,
};

/// Geometry a headless spawn uses when the session has no live pane to copy a size from.
///
/// A pane spawned with no client attached has no layout to measure against. The first controller
/// to attach resizes it, so this only has to be a sane size for whatever the pane runs in the
/// meantime rather than a guess at anyone's terminal.
const HEADLESS_SPAWN_COLS: u16 = DEFAULT_COLS;
const HEADLESS_SPAWN_ROWS: u16 = DEFAULT_ROWS;

/// `author` on a [`ServerMessage::LayoutCommitted`] the server wrote itself.
///
/// Client ids are handed out from 1 (see `SessionServer::new_named_with_settings`), so this can
/// never collide with a real client and no client mistakes a server-authored revision for the
/// echo of its own commit.
const SERVER_LAYOUT_AUTHOR: ClientId = 0;

/// One pane as `list-panes` reports it.
///
/// Field-for-field the shape `src/ops/control.rs` produces from a client's `State`, so
/// `rozi list-panes` renders one table and a script parses one document whichever endpoint
/// answered. The values come from different places — this side reads the authoritative server
/// runtime state directly instead of the copy a client keeps — but the contract is the CLI's, not
/// either implementation's.
#[derive(Serialize)]
struct SessionPaneInfo {
    session: String,
    id: PaneId,
    title: String,
    /// One-based workspace from the shared layout, or `0` when the session has no layout document
    /// yet (nothing has placed the pane, so there is no workspace to name).
    workspace: usize,
    command: Option<String>,
    argv: Option<Vec<String>>,
    foreground_program: Option<String>,
    foreground_programs: Vec<String>,
    foreground_arguments: Vec<String>,
    cwd: Option<String>,
    /// Lifecycle text in the same vocabulary a client's terminal reports: `ready`, or
    /// `exited (N)` for a pane whose process is gone but whose screen is still readable.
    status: String,
    reported_status: Option<String>,
    status_reason: Option<String>,
    agent: Option<String>,
    agent_state: Option<String>,
}

#[derive(Serialize)]
struct SessionPaneCapture {
    id: PaneId,
    text: String,
    title: Option<String>,
}

#[derive(Serialize)]
struct SessionNewPane {
    id: PaneId,
    accepted: bool,
    pty_ready: bool,
}

/// Why a control command cannot be served by a session server.
///
/// Spelled out per command rather than as one blanket "unsupported": a script that reaches for
/// `rozi --session dev focus 3` is asking for something that does not exist off-screen, and the
/// message has to say that rather than suggest the session is broken.
pub fn session_control_unsupported(command: &ControlCommand) -> Option<&'static str> {
    match command {
        ControlCommand::ListPanes
        | ControlCommand::Metrics
        | ControlCommand::SendText { .. }
        | ControlCommand::SendKeys { .. }
        | ControlCommand::NewPane { .. }
        | ControlCommand::CapturePane { .. }
        | ControlCommand::PaneLogging { .. }
        | ControlCommand::SetStatus { .. } => None,
        ControlCommand::Focus { .. } => Some(
            "focus is client-local; a session server has no focused pane to move (every headless command names its pane with --target instead)",
        ),
        ControlCommand::SwitchWorkspace { .. } | ControlCommand::MoveToWorkspace { .. } => Some(
            "the active workspace is client-local; a session server cannot switch it (use `split --workspace` to place a pane)",
        ),
        ControlCommand::RunAction { .. } => {
            Some("actions run inside a UI; a session server has no action to run")
        }
        ControlCommand::Popup { .. } => {
            Some("popups are drawn by a UI; a session server cannot open one")
        }
        ControlCommand::Notify { .. } => {
            Some("toasts are drawn by a UI; a session server has nowhere to show one")
        }
        ControlCommand::Pick { .. } => {
            Some("pick opens a modal in a UI; a session server cannot show one")
        }
        ControlCommand::Publish => Some(
            "publish belongs to the pane that publishes; run it inside the pane, which reaches its own session",
        ),
        ControlCommand::Subscribe { .. } => Some(
            "subscribe streams UI events; a session server does not raise them (poll `list-panes` for pane state)",
        ),
    }
}

impl SessionServer {
    /// Answer one headless control request.
    ///
    /// Returns the reply plus whatever the command changed for everyone else. A session with no
    /// clients broadcasts into an empty room, which is the ordinary case here; a session someone is
    /// also watching sees a headless spawn or status change exactly as it sees a client's.
    pub(super) fn handle_session_control(
        &mut self,
        session: String,
        protocol_version: u32,
        min_protocol_version: u32,
        capabilities: Option<protocol::Capabilities>,
        request: ControlRequest,
    ) -> Vec<(Target, ServerMessage)> {
        let effective = match protocol::negotiate_protocol(
            protocol_version,
            min_protocol_version,
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
        ) {
            Ok(effective) => effective,
            Err(mismatch) => {
                return vec![(
                    Target::Sender,
                    ServerMessage::Error {
                        code: "protocol-mismatch".to_string(),
                        message: mismatch.message(),
                    },
                )];
            }
        };
        if session != self.session_name {
            return vec![(
                Target::Sender,
                ServerMessage::Error {
                    code: "session-mismatch".to_string(),
                    message: format!(
                        "client requested session {session:?}, but this server owns {:?}",
                        self.session_name
                    ),
                },
            )];
        }
        let capabilities = protocol::Capabilities::negotiated(capabilities.as_ref());
        let mut broadcasts = Vec::new();
        let response = self.run_session_control(request, &mut broadcasts);
        let mut messages = vec![(
            Target::Sender,
            ServerMessage::SessionControlResult {
                capabilities: Some(capabilities),
                effective_protocol: effective,
                response,
            },
        )];
        messages.extend(broadcasts);
        messages
    }

    fn run_session_control(
        &mut self,
        request: ControlRequest,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        if let Some(reason) = session_control_unsupported(&request.command) {
            return ControlResponse::error(reason);
        }
        let source_pane = request.source_pane;
        match request.command {
            ControlCommand::ListPanes => ControlResponse::ok(self.session_pane_report()),
            // A headless sample is taken now rather than read from a client's cache, so it is
            // never stale; the wrapper keeps the document shape `rozi metrics` already renders.
            ControlCommand::Metrics => ControlResponse::ok(serde_json::json!({
                "sampled_at_unix_ms": crate::runtime_metrics::unix_time_millis(),
                "server": crate::runtime_metrics::CachedServerRuntimeMetrics {
                    sample: self.runtime_metrics(),
                    age_ms: 0,
                    stale: false,
                },
            })),
            ControlCommand::CapturePane { target, scrollback } => {
                self.session_capture_pane(target.or(source_pane), scrollback)
            }
            ControlCommand::SendText { target, text } => {
                self.session_send_bytes(target.or(source_pane), text.into_bytes())
            }
            ControlCommand::SendKeys {
                target,
                keys,
                literal,
            } => self.session_send_keys(target.or(source_pane), &keys, literal),
            ControlCommand::NewPane {
                command,
                argv,
                cwd,
                title,
                keep_open,
                focus,
                workspace,
            } => self.session_new_pane(
                SessionSpawn {
                    command,
                    argv,
                    cwd,
                    title,
                    keep_open,
                    focus,
                    workspace,
                },
                broadcasts,
            ),
            ControlCommand::SetStatus {
                target,
                status,
                reason,
            } => self.session_set_status(target.or(source_pane), status, reason, broadcasts),
            ControlCommand::PaneLogging { target, enabled } => {
                self.session_pane_logging(target.or(source_pane), enabled, broadcasts)
            }
            // Every remaining variant was refused above by `session_control_unsupported`.
            other => ControlResponse::error(
                session_control_unsupported(&other).unwrap_or("unsupported control command"),
            ),
        }
    }

    fn session_pane_report(&self) -> Vec<SessionPaneInfo> {
        let workspaces = self.layout_workspace_index();
        let mut panes: Vec<SessionPaneInfo> = self
            .panes
            .iter()
            .map(|(id, pane)| SessionPaneInfo {
                session: self.session_name.clone(),
                id: *id,
                title: pane
                    .effective_title()
                    .unwrap_or_else(|| format!("pane {id}")),
                workspace: workspaces.get(id).copied().unwrap_or(0),
                command: pane
                    .launch
                    .as_ref()
                    .and_then(crate::pane::launch::PaneLaunch::shell_command)
                    .map(str::to_string),
                argv: pane
                    .launch
                    .as_ref()
                    .and_then(crate::pane::launch::PaneLaunch::argv)
                    .map(<[String]>::to_vec),
                foreground_program: pane.runtime.foreground_program.clone(),
                foreground_programs: pane.runtime.foreground_programs.to_vec(),
                foreground_arguments: pane.runtime.foreground_arguments.clone(),
                cwd: pane.runtime.cwd.clone().or_else(|| pane.cwd.clone()),
                status: match pane.exited {
                    None => "ready".to_string(),
                    Some(code) => format!("exited ({code})"),
                },
                reported_status: pane
                    .runtime
                    .status
                    .as_ref()
                    .map(|status| status.value.clone()),
                status_reason: pane
                    .runtime
                    .status
                    .as_ref()
                    .and_then(|status| status.reason.clone()),
                agent: pane
                    .runtime
                    .detected_agent
                    .as_ref()
                    .map(|detected| detected.agent.id.clone()),
                agent_state: pane
                    .runtime
                    .detected_agent
                    .as_ref()
                    .map(|detected| protocol::detected_agent_status(detected).to_string()),
            })
            .collect();
        // A HashMap iteration order would reshuffle the table between two identical calls.
        panes.sort_by_key(|pane| pane.id);
        panes
    }

    /// One-based workspace number per pane, read from the shared layout document.
    fn layout_workspace_index(&self) -> HashMap<PaneId, usize> {
        let mut index = HashMap::new();
        let Some(layout) = &self.layout else {
            return index;
        };
        for workspace in &layout.workspaces {
            for pane in &workspace.panes {
                index.insert(pane.pane_id, workspace.index + 1);
            }
        }
        index
    }

    /// The pane a headless command without an explicit target addresses.
    ///
    /// There is no focused pane to fall back to, so a session with exactly one pane resolves to it
    /// and anything else is an error naming the ids to choose from. Guessing would be worse than
    /// failing: typing into the wrong pane of a detached session is invisible until someone
    /// attaches and finds it.
    fn session_target_pane(
        &self,
        target: Option<PaneId>,
    ) -> std::result::Result<PaneId, ControlResponse> {
        if let Some(id) = target {
            return if self.panes.contains_key(&id) {
                Ok(id)
            } else {
                Err(ControlResponse::error(format!("pane {id} not found")))
            };
        }
        let mut ids: Vec<PaneId> = self.panes.keys().copied().collect();
        ids.sort_unstable();
        match ids.as_slice() {
            [only] => Ok(*only),
            [] => Err(ControlResponse::error(format!(
                "session `{}` has no panes",
                self.session_name
            ))),
            many => Err(ControlResponse::error(format!(
                "session `{}` has {} panes and no focused pane; pass --target (ids: {})",
                self.session_name,
                many.len(),
                many.iter()
                    .map(PaneId::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))),
        }
    }

    fn session_capture_pane(
        &mut self,
        target: Option<PaneId>,
        scrollback: Option<CaptureScrollback>,
    ) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        let Some(pane) = self.panes.get_mut(&id) else {
            return ControlResponse::error(format!("pane {id} not found"));
        };
        // Reading a snapshot does not change what a replay would contain, so this must not bump
        // `content_generation` and make every snapshot re-export the pane it just captured.
        let screen = pane.screen_without_change();
        let text = match scrollback {
            None => screen.render_snapshot().text.to_string(),
            Some(CaptureScrollback::Lines(lines)) => {
                let total = screen.total_text_lines();
                screen.export_text(total.saturating_sub(lines), total)
            }
            Some(CaptureScrollback::Named(CaptureScrollbackNamed::Full)) => {
                let total = screen.total_text_lines();
                screen.export_text(0, total)
            }
            Some(CaptureScrollback::Named(CaptureScrollbackNamed::LastOutput)) => {
                match screen.export_last_command_output() {
                    Some(text) => text,
                    None => {
                        return ControlResponse::error(
                            "no last command output (shell integration marks missing)",
                        );
                    }
                }
            }
        };
        let title = pane.screen().title();
        ControlResponse::ok(SessionPaneCapture { id, text, title })
    }

    fn session_send_bytes(&mut self, target: Option<PaneId>, bytes: Vec<u8>) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        // The input lock is the session saying nobody but its controller types right now — a
        // presenter's guard against the audience. A headless caller is not the controller and has
        // no screen to notice it, so it is refused rather than quietly allowed through.
        if self.input_locked {
            return ControlResponse::error(format!(
                "session `{}` has input locked; unlock it from the attached client",
                self.session_name
            ));
        }
        let Some(pane) = self.panes.get(&id) else {
            return ControlResponse::error(format!("pane {id} not found"));
        };
        if pane.exited.is_some() || pane.pty.is_none() {
            return ControlResponse::error(format!("pane {id} PTY is not running"));
        }
        let generation = pane.generation;
        self.handle_pane_input(None, id, generation, &bytes);
        ControlResponse::empty()
    }

    fn session_send_keys(
        &mut self,
        target: Option<PaneId>,
        keys: &[String],
        literal: bool,
    ) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        let Some(pane) = self.panes.get(&id) else {
            return ControlResponse::error(format!("pane {id} not found"));
        };
        // The server's own parser holds the child's key modes, so `C-c` and the arrow keys encode
        // against what the program actually enabled rather than a default.
        let modes = pane.screen().key_modes();
        // Encode everything before writing anything, so an unrepresentable key later in the batch
        // cannot leave half a command line in the pane.
        let mut bytes = Vec::new();
        for key in keys {
            match crate::input::send_keys::parse_send_keys_arg(key, literal) {
                Ok(crate::input::send_keys::SendKeysItem::Text(text)) => {
                    bytes.extend(text.into_bytes());
                }
                Ok(crate::input::send_keys::SendKeysItem::Key(event)) => {
                    let Some(encoded) =
                        crate::pane::pty_events::terminal_key_event_bytes(event, modes)
                    else {
                        return ControlResponse::error(
                            "key is not representable for session forwarding yet",
                        );
                    };
                    bytes.extend(encoded);
                }
                Err(message) => return ControlResponse::error(message),
            }
        }
        self.session_send_bytes(Some(id), bytes)
    }

    fn session_set_status(
        &mut self,
        target: Option<PaneId>,
        status: Option<String>,
        reason: Option<String>,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        let Some(generation) = self.panes.get(&id).map(|pane| pane.generation) else {
            return ControlResponse::error(format!("pane {id} not found"));
        };
        match self.apply_pane_status(None, id, generation, status, reason) {
            Ok(Some(state)) => {
                broadcasts.push((
                    Target::Broadcast,
                    ServerMessage::PaneRuntimeChanged {
                        pane_id: id,
                        local: false,
                        generation,
                        state,
                    },
                ));
                ControlResponse::empty()
            }
            Ok(None) => ControlResponse::empty(),
            Err((_, message)) => ControlResponse::error(message),
        }
    }

    fn session_pane_logging(
        &mut self,
        target: Option<PaneId>,
        enabled: Option<bool>,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        let Some(pane) = self.panes.get(&id) else {
            return ControlResponse::error(format!("pane {id} not found"));
        };
        let generation = pane.generation;
        let enabled = enabled.unwrap_or(pane.log.is_none());
        let message = self.set_pane_logging(None, id, generation, enabled);
        let data = match &message {
            ServerMessage::PaneLoggingChanged {
                enabled,
                path,
                error: None,
                ..
            } => serde_json::json!({
                "id": id,
                "enabled": enabled,
                "path": path,
            }),
            ServerMessage::PaneLoggingChanged {
                error: Some(error), ..
            } => return ControlResponse::error(error.clone()),
            _ => serde_json::Value::Null,
        };
        broadcasts.push((Target::Broadcast, message));
        ControlResponse::ok(data)
    }

    fn session_new_pane(
        &mut self,
        spawn: SessionSpawn,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        if spawn.focus {
            return ControlResponse::error(
                "split --focus needs a UI; a session server has no focus to move",
            );
        }
        let workspace_index = match spawn.workspace {
            None => 0,
            Some(index) if (1..=crate::state::WORKSPACE_COUNT).contains(&index) => index - 1,
            Some(index) => {
                return ControlResponse::error(format!(
                    "workspace {index} is out of range (1-{})",
                    crate::state::WORKSPACE_COUNT
                ));
            }
        };
        let launch = match (spawn.command, spawn.argv) {
            (Some(_), Some(_)) => {
                return ControlResponse::error("split accepts either COMMAND or --argv, not both");
            }
            (Some(command), None) => Some(crate::pane::launch::PaneLaunch::shell(command)),
            (None, Some(argv)) => match crate::pane::launch::PaneLaunch::direct(argv) {
                Ok(launch) => Some(launch),
                Err(error) => return ControlResponse::error(error),
            },
            (None, None) => None,
        };
        // A layout is the only thing that makes a pane visible to a client, and a session that has
        // panes but no layout document is one whose panes were placed by something this server
        // cannot reconstruct. Adding a pane there would commit a document claiming the others do
        // not exist, so refuse instead and say what would fix it.
        if self.layout.is_none() && !self.panes.is_empty() {
            return ControlResponse::error(format!(
                "session `{}` has panes but no shared layout; attach a client once so it commits one",
                self.session_name
            ));
        }

        let Some(pane_id) = self.next_headless_pane_id() else {
            return ControlResponse::error(format!(
                "session `{}` has no free pane id below the reserved {}",
                self.session_name,
                crate::state::POPUP_PANE_ID
            ));
        };
        let generation = self.next_generation;
        self.next_generation += 1;
        let (cols, rows) = self.headless_spawn_size();
        let palette = self
            .panes
            .values()
            .next()
            .map(|pane| pane.palette)
            .unwrap_or_else(|| WirePalette::from(tui_lipan::TerminalColorPalette::default()));
        let shell = self.settings.shell.clone();
        let command_shell = self.settings.command_shell.clone();
        let result = self.spawn_pane(SpawnRequest {
            pane_id,
            owner: None,
            generation,
            launch: launch.clone(),
            cwd: spawn.cwd.clone(),
            title: spawn.title.clone(),
            cols,
            rows,
            keep_open: spawn.keep_open,
            // A headless spawn has no UI to advertise. `ROZI_SOCKET` and `ROZI_BIN` name a
            // client process and there is not one, so the child gets the same two variables a
            // remote pane does rather than a path that resolves to nothing or to some unrelated
            // rozi that happens to be running.
            env: vec![
                ("ROZI".to_string(), "1".to_string()),
                ("ROZI_PANE".to_string(), pane_id.to_string()),
            ],
            palette,
            shell,
            command_shell,
            cell: None,
        });
        // `SpawnResult` answers the client that asked; a follower learns about a new pane from
        // the layout revision below, exactly as it does for a controller's split. Broadcasting it
        // would hand every client a reply to a request it never made.
        let ok = match result {
            ServerMessage::SpawnResult { ok: true, .. } => true,
            ServerMessage::SpawnResult { error, .. } => {
                return ControlResponse::error(error.unwrap_or_else(|| "spawn failed".to_string()));
            }
            _ => false,
        };

        if let Some(layout) = self.headless_layout_with_pane(
            pane_id,
            generation,
            workspace_index,
            launch,
            spawn.cwd,
            spawn.title,
            spawn.keep_open,
        ) {
            self.layout_rev += 1;
            self.layout = Some(layout.clone());
            self.mark_dirty();
            broadcasts.push((
                Target::Broadcast,
                ServerMessage::LayoutCommitted {
                    rev: self.layout_rev,
                    // Client ids start at 1, so `0` is a document no client authored. Every
                    // client, the controller included, therefore reconciles this revision instead
                    // of recognising it as its own echo — which is right: none of them wrote it.
                    // The lease does not move; the controller keeps control throughout.
                    author: SERVER_LAYOUT_AUTHOR,
                    layout,
                },
            ));
        }

        ControlResponse::ok(SessionNewPane {
            id: pane_id,
            accepted: true,
            pty_ready: ok,
        })
    }

    /// A pane id no live pane, exited pane, or layout entry is using.
    ///
    /// Always above every id in use rather than filling a gap a closed pane left. Clients raise
    /// their own counter past whatever the shared layout carries when they reconcile, so climbing
    /// is what guarantees the next client-side spawn cannot land on this id; reusing a gap would
    /// also hand a stale reference to a pane that is not the one it meant.
    ///
    /// `None` once the climb would reach [`crate::state::POPUP_PANE_ID`], which is
    /// [`PaneId::MAX`] and reserved: there is no id above it, so the honest answer is that the
    /// session has none left rather than a reserved id a layout would reject.
    fn next_headless_pane_id(&self) -> Option<PaneId> {
        let highest = self
            .panes
            .keys()
            .copied()
            .chain(self.local_panes.keys().map(|(_, id)| *id))
            .chain(
                self.layout
                    .iter()
                    .flat_map(|layout| &layout.workspaces)
                    .flat_map(|workspace| &workspace.panes)
                    .map(|pane| pane.pane_id),
            )
            .max()
            .unwrap_or(0);
        highest
            .checked_add(1)
            .filter(|next| *next != crate::state::POPUP_PANE_ID)
    }

    /// Geometry for a headless spawn: whatever the session's panes are already using, so a new pane
    /// lands the same size as its neighbours instead of forcing a reflow when a client attaches.
    fn headless_spawn_size(&self) -> (u16, u16) {
        self.panes
            .values()
            .find(|pane| pane.exited.is_none())
            .map(|pane| (pane.cols, pane.rows))
            .unwrap_or((HEADLESS_SPAWN_COLS, HEADLESS_SPAWN_ROWS))
    }

    /// The shared layout with the new pane appended to `workspace_index`.
    ///
    /// The workspace's tiling tree is deliberately left alone: a client places a pane missing from
    /// the tree through `effective_tile_tree`, which appends it in the workspace's own start axis.
    /// Splitting the tree here instead would have the server guess at a geometry no one asked for
    /// and overwrite a deliberate ratio.
    #[allow(clippy::too_many_arguments)]
    fn headless_layout_with_pane(
        &self,
        pane_id: PaneId,
        generation: u64,
        workspace_index: usize,
        launch: Option<crate::pane::launch::PaneLaunch>,
        cwd: Option<String>,
        title: Option<String>,
        keep_open: bool,
    ) -> Option<SharedLayout> {
        let shared_pane = SharedPane {
            pane_id,
            generation,
            title,
            profile_name: None,
            cwd,
            launch,
            replay: false,
            keep_open,
            floating: false,
            fullscreen: false,
            rect: None::<FracRect>,
            scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
        };
        let mut layout = self.layout.clone().unwrap_or_else(|| SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: HEADLESS_SPAWN_COLS,
            canvas_rows: HEADLESS_SPAWN_ROWS,
            workspaces: Vec::new(),
        });
        match layout
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.index == workspace_index)
        {
            Some(workspace) => workspace.panes.push(shared_pane.clone()),
            None => layout.workspaces.push(SharedWorkspace {
                index: workspace_index,
                name: None,
                synchronized: false,
                layout: crate::layout::shared::SharedLayoutKind::default(),
                start_axis: crate::layout::shared::SharedSplitAxis::default(),
                split_ratios: Vec::new(),
                tree: None,
                panes: vec![shared_pane],
            }),
        }
        // A document the server would itself reject is worse than no document: the pane exists
        // either way, and leaving the old revision in place keeps every attached client consistent.
        layout.validate().ok()?;
        Some(layout)
    }
}

/// What a headless `split` asked for.
struct SessionSpawn {
    command: Option<String>,
    argv: Option<Vec<String>>,
    cwd: Option<String>,
    title: Option<String>,
    keep_open: bool,
    focus: bool,
    workspace: Option<usize>,
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use crate::control::ControlRequest;
    use crate::layout::shared::{SharedSplitAxis, SharedTree};

    fn request(command: ControlCommand) -> ControlRequest {
        ControlRequest {
            command,
            source_pane: None,
            extension: None,
        }
    }

    /// Run a headless command against `server` and return its `{ok, data, error}` answer plus
    /// everything else the command broadcast.
    fn control(
        server: &mut SessionServer,
        command: ControlCommand,
    ) -> (ControlResponse, Vec<(Target, ServerMessage)>) {
        let mut messages = server.handle_session_control(
            server.session_name.clone(),
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
            None,
            request(command),
        );
        let response = match messages.remove(0) {
            (Target::Sender, ServerMessage::SessionControlResult { response, .. }) => response,
            other => panic!("expected a control result, got {other:?}"),
        };
        (response, messages)
    }

    fn pane_with_screen(server: &mut SessionServer, id: PaneId, bytes: &[u8]) {
        let mut pane = super::super::tests::test_pane(1);
        pane.screen_mut().process_bytes(bytes);
        server.panes.insert(id, pane);
    }

    fn one_pane_layout(pane_id: PaneId) -> SharedLayout {
        SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: 80,
            canvas_rows: 24,
            workspaces: vec![SharedWorkspace {
                index: 0,
                name: None,
                synchronized: false,
                layout: crate::layout::shared::SharedLayoutKind::default(),
                start_axis: SharedSplitAxis::default(),
                split_ratios: Vec::new(),
                tree: Some(SharedTree::Leaf { pane: pane_id }),
                panes: vec![SharedPane {
                    pane_id,
                    generation: 1,
                    title: None,
                    profile_name: None,
                    cwd: None,
                    launch: None,
                    replay: false,
                    keep_open: false,
                    floating: false,
                    fullscreen: false,
                    rect: None,
                    scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
                }],
            }],
        }
    }

    #[test]
    fn a_headless_request_for_another_session_is_refused_before_anything_runs() {
        let mut server = SessionServer::new_named("dev");
        let messages = server.handle_session_control(
            "other".to_string(),
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
            None,
            request(ControlCommand::ListPanes),
        );
        assert!(matches!(
            messages.as_slice(),
            [(Target::Sender, ServerMessage::Error { code, .. })] if code == "session-mismatch"
        ));
    }

    #[test]
    fn a_headless_request_from_an_incompatible_build_is_refused_at_the_handshake() {
        let mut server = SessionServer::new_named("dev");
        let messages = server.handle_session_control(
            "dev".to_string(),
            PROTOCOL_VERSION.saturating_sub(1),
            PROTOCOL_VERSION.saturating_sub(1),
            None,
            request(ControlCommand::ListPanes),
        );
        assert!(matches!(
            messages.as_slice(),
            [(Target::Sender, ServerMessage::Error { code, .. })] if code == "protocol-mismatch"
        ));
    }

    #[test]
    fn capture_reads_the_server_screen_without_marking_the_pane_for_re_export() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 7, b"hello\r\n");
        let before = server.panes[&7].content_generation;

        let (response, _) = control(
            &mut server,
            ControlCommand::CapturePane {
                target: Some(7),
                scrollback: None,
            },
        );
        assert!(response.ok, "{:?}", response.error);
        let data = response.data.expect("capture returns text");
        assert!(
            data["text"]
                .as_str()
                .is_some_and(|text| text.contains("hello")),
            "{data}"
        );
        // Capturing is a read. Bumping `content_generation` would make the next snapshot
        // re-export a pane whose replay bytes did not change.
        assert_eq!(server.panes[&7].content_generation, before);
    }

    #[test]
    fn a_locked_session_refuses_headless_input_and_still_answers_reads() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 3, b"idle\r\n");
        server.input_locked = true;

        let (refused, _) = control(
            &mut server,
            ControlCommand::SendText {
                target: Some(3),
                text: "rm -rf /\n".to_string(),
            },
        );
        assert!(!refused.ok);
        assert!(
            refused
                .error
                .unwrap_or_default()
                .contains("has input locked"),
            "a locked session must say why it refused"
        );

        let (listed, _) = control(&mut server, ControlCommand::ListPanes);
        assert!(listed.ok, "a lock stops typing, not looking");
    }

    #[test]
    fn a_headless_spawn_appends_to_the_layout_without_disturbing_the_tiling_tree() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 4, b"");
        server.layout = Some(one_pane_layout(4));
        server.layout_rev = 9;
        server.next_generation = 2;

        let (response, broadcasts) = control(
            &mut server,
            ControlCommand::NewPane {
                command: None,
                argv: None,
                cwd: None,
                title: Some("worker".to_string()),
                keep_open: false,
                focus: false,
                workspace: None,
            },
        );
        assert!(response.ok, "{:?}", response.error);
        let id = response.data.expect("spawn data")["id"]
            .as_u64()
            .expect("spawn reports an id") as PaneId;
        assert_eq!(id, 5, "a headless id follows the highest id in use");

        let layout = match broadcasts.as_slice() {
            [
                (
                    Target::Broadcast,
                    ServerMessage::LayoutCommitted {
                        rev,
                        author,
                        layout,
                    },
                ),
            ] => {
                assert_eq!(*rev, 10);
                assert_eq!(*author, SERVER_LAYOUT_AUTHOR);
                layout.clone()
            }
            other => panic!("expected exactly one layout commit, got {other:?}"),
        };
        let workspace = &layout.workspaces[0];
        assert_eq!(
            workspace
                .panes
                .iter()
                .map(|pane| pane.pane_id)
                .collect::<Vec<_>>(),
            vec![4, 5]
        );
        // The workspace's own tree is left exactly as the controller arranged it; a client places
        // the new leaf through `effective_tile_tree` rather than inheriting a guess made here.
        assert_eq!(workspace.tree, Some(SharedTree::Leaf { pane: 4 }));
        layout.validate().expect("the committed document is valid");
    }

    #[test]
    fn a_headless_spawn_is_refused_when_the_session_has_panes_but_no_layout_to_place_them_in() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 2, b"");
        assert!(server.layout.is_none());

        let (response, broadcasts) = control(
            &mut server,
            ControlCommand::NewPane {
                command: None,
                argv: None,
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: None,
            },
        );
        assert!(!response.ok);
        assert!(
            response
                .error
                .unwrap_or_default()
                .contains("no shared layout"),
            "the refusal must name the missing layout"
        );
        assert!(
            broadcasts.is_empty(),
            "a refused spawn must not touch any client"
        );
        assert_eq!(server.panes.len(), 1, "and must not leave a pane behind");
    }

    #[test]
    fn a_headless_spawn_refuses_rather_than_minting_the_reserved_popup_pane_id() {
        let mut server = SessionServer::new_named("dev");
        let last = crate::state::POPUP_PANE_ID - 1;
        pane_with_screen(&mut server, last, b"");
        server.layout = Some(one_pane_layout(last));
        assert_eq!(
            server.next_headless_pane_id(),
            None,
            "the popup id is reserved, and a layout carrying it would be rejected"
        );

        let (response, broadcasts) = control(
            &mut server,
            ControlCommand::NewPane {
                command: None,
                argv: None,
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: None,
            },
        );
        assert!(!response.ok);
        assert!(
            response.error.unwrap_or_default().contains("free pane id"),
            "exhaustion must be reported, not worked around"
        );
        assert!(broadcasts.is_empty());
    }

    #[test]
    fn every_ui_only_command_is_refused_with_a_reason_rather_than_silently_accepted() {
        // A new `ControlCommand` variant lands in exactly one of the two halves of
        // `session_control_unsupported`, and this is what forces the author to choose.
        for command in [
            ControlCommand::Focus { target: 1 },
            ControlCommand::RunAction {
                action: "toggle-float".to_string(),
            },
            ControlCommand::SwitchWorkspace { index: 1 },
            ControlCommand::MoveToWorkspace { index: 1 },
            ControlCommand::Popup {
                command: "top".to_string(),
                cwd: None,
                width: None,
                height: None,
                title: None,
                keep_open: None,
            },
            ControlCommand::Notify {
                message: "hi".to_string(),
                title: None,
                level: crate::control::NotifyLevel::Info,
            },
            ControlCommand::Pick {
                title: None,
                placeholder: None,
                width: None,
                actions: Vec::new(),
            },
            ControlCommand::Publish,
            ControlCommand::Subscribe { events: Vec::new() },
        ] {
            let reason = session_control_unsupported(&command)
                .unwrap_or_else(|| panic!("{command:?} must be refused by a session server"));
            assert!(
                !reason.is_empty() && reason.contains(' '),
                "{command:?} needs a sentence, not a code: {reason}"
            );
        }
    }
}
