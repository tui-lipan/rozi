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
    AgentInfo, AgentTarget, CaptureScrollback, ControlCommand, ControlErrorCode, ControlRequest,
    ControlResponse,
};
use crate::layout::shared::{
    SHARED_LAYOUT_VERSION, SharedLayout, SharedPane, SharedWorkspace, float_rect_to_frac,
};

/// Geometry a headless spawn uses when the session has no live pane to copy a size from.
///
/// A pane spawned with no client attached has no layout to measure against. The first controller
/// to attach resizes it, so this only has to be a sane size for whatever the pane runs in the
/// meantime rather than a guess at anyone's terminal.
const HEADLESS_SPAWN_COLS: u16 = DEFAULT_COLS;
const HEADLESS_SPAWN_ROWS: u16 = DEFAULT_ROWS;

/// Where a headless `split` lands when neither the caller nor a `[[rules]]` entry named a
/// workspace. A client would use the one it is looking at; there is nothing to look at here, so
/// the first workspace is the only answer that is the same on every run.
const HEADLESS_DEFAULT_WORKSPACE: usize = 0;

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
    reference: protocol::PaneRef,
    agent_ref: Option<protocol::AgentRef>,
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

struct SessionAgentPrompt<'a> {
    target: AgentTarget,
    prompt: &'a str,
    wait: Option<crate::control::AgentWaitCondition>,
    timeout_ms: Option<u64>,
    allow_working: bool,
}

/// Why a control command cannot be served by a session server.
///
/// Spelled out per command rather than as one blanket "unsupported": a script that reaches for
/// `rozi --session dev focus 3` is asking for something that does not exist off-screen, and the
/// message has to say that rather than suggest the session is broken.
pub fn session_control_unsupported(command: &ControlCommand) -> Option<&'static str> {
    match command {
        ControlCommand::ListPanes
        | ControlCommand::AgentsList
        | ControlCommand::AgentGet { .. }
        | ControlCommand::AgentRead { .. }
        | ControlCommand::Metrics
        | ControlCommand::SendText { .. }
        | ControlCommand::SendKeys { .. }
        | ControlCommand::NewPane { .. }
        | ControlCommand::CapturePane { .. }
        | ControlCommand::PaneLogging { .. }
        | ControlCommand::SetStatus { .. }
        | ControlCommand::AgentWait { .. }
        | ControlCommand::AgentPrompt { .. }
        | ControlCommand::AgentReport { .. }
        | ControlCommand::AgentRelease { .. } => None,
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

/// Why a session server cannot accept a request carrying extension provenance.
///
/// The generation is a fencing token a *client* mints on each config reload and injects into the
/// extension's environment, so a retired runtime definition's leftover processes stop being obeyed
/// (see [`crate::config::provenance_is_active`], which the UI endpoint checks first thing). A
/// session server never sees that token: it is minted per client process, and the server's own
/// config load would produce a different one.
///
/// So the choice is between honouring a token nobody checked and refusing. Accepting would quietly
/// turn `--session` into the way around a fence the UI endpoint enforces - a disabled extension's
/// `send-text` would keep landing in panes. Refusing costs an extension the headless path until
/// there is a real way to validate it, and says so rather than failing obscurely.
fn unverifiable_extension_provenance(provenance: &crate::config::ExtensionProvenance) -> String {
    format!(
        "a session server cannot check whether extension `{}` is still active, and will not act on its behalf; reach a running rozi instead, or clear ROZI_EXTENSION when the caller is not the extension",
        provenance.id
    )
}

impl SessionServer {
    /// Answer one headless control request.
    ///
    /// Returns the reply plus whatever the command changed for everyone else. A session with no
    /// clients broadcasts into an empty room, which is the ordinary case here; a session someone is
    /// also watching sees a headless spawn or status change exactly as it sees a client's.
    pub(super) fn handle_session_control(
        &mut self,
        client_id: ClientId,
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
        if let ControlCommand::AgentWait {
            target,
            until,
            timeout_ms,
        } = &request.command
        {
            let response = if let Some(provenance) = &request.extension {
                Some(ControlResponse::error(unverifiable_extension_provenance(
                    provenance,
                )))
            } else {
                self.register_agent_wait(
                    client_id,
                    target.clone(),
                    *until,
                    *timeout_ms,
                    capabilities.clone(),
                    effective,
                )
            };
            return response.map_or_else(Vec::new, |response| {
                vec![(
                    Target::Sender,
                    ServerMessage::SessionControlResult {
                        capabilities: Some(capabilities),
                        effective_protocol: effective,
                        response,
                    },
                )]
            });
        }
        if let ControlCommand::AgentPrompt {
            target,
            prompt,
            wait,
            timeout_ms,
            allow_working,
        } = &request.command
        {
            let response = if let Some(provenance) = &request.extension {
                Some(ControlResponse::error(unverifiable_extension_provenance(
                    provenance,
                )))
            } else {
                self.register_agent_prompt(
                    client_id,
                    SessionAgentPrompt {
                        target: target.clone(),
                        prompt,
                        wait: *wait,
                        timeout_ms: *timeout_ms,
                        allow_working: *allow_working,
                    },
                    capabilities.clone(),
                    effective,
                )
            };
            return response.map_or_else(Vec::new, |response| {
                vec![(
                    Target::Sender,
                    ServerMessage::SessionControlResult {
                        capabilities: Some(capabilities),
                        effective_protocol: effective,
                        response,
                    },
                )]
            });
        }
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
        if let Some(provenance) = &request.extension {
            return ControlResponse::error(unverifiable_extension_provenance(provenance));
        }
        if let Some(reason) = session_control_unsupported(&request.command) {
            return ControlResponse::error_with(ControlErrorCode::Unsupported, reason);
        }
        // `source_pane` is deliberately not read here, and the CLI does not send it to a session
        // endpoint either.
        //
        // It carries a bare pane id taken from the caller's `ROZI_PANE`, with nothing saying which
        // session that id belongs to. `--session` changes the namespace, so a script run inside
        // pane 3 of `work` that says `--session dev` would arrive claiming pane 3 - and `dev`'s
        // pane 3 is a different pane, in a session the caller never looked at. Worse, an inherited
        // id looks exactly like an explicit `--target`, so the ambiguity check below never runs and
        // a five-pane session gets typed into silently instead of refusing.
        //
        // Honouring it would need a session identity beside the pane id. A spawn-time
        // `ROZI_SESSION` is the obvious shape and is not safe as written: a session can be renamed
        // (`ClientMessage::Rename`) long after a pane's environment was fixed, and a stale value
        // could later match a *different* session that took the old name. Until that is designed,
        // a pane naming a pane in its own session says so with `--target "$ROZI_PANE"`.
        match request.command {
            ControlCommand::ListPanes => ControlResponse::ok(self.session_pane_report()),
            ControlCommand::AgentsList => ControlResponse::ok(self.session_agent_report()),
            ControlCommand::AgentGet { target } => self.session_agent_get(target),
            ControlCommand::AgentRead { target, scrollback } => {
                match self.resolve_agent_wait_target(target) {
                    Ok(reference) => match self.validate_agent_input_reference(&reference) {
                        Ok(()) => {
                            self.session_capture_pane(Some(reference.pane.pane_id), scrollback)
                        }
                        Err(response) => response,
                    },
                    Err(response) => response,
                }
            }
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
                self.session_capture_pane(target, scrollback)
            }
            ControlCommand::SendText { target, text } => {
                self.session_send_bytes(target, text.into_bytes())
            }
            ControlCommand::SendKeys {
                target,
                keys,
                literal,
            } => self.session_send_keys(target, &keys, literal),
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
            } => self.session_set_status(target, status, reason, broadcasts),
            ControlCommand::PaneLogging { target, enabled } => {
                self.session_pane_logging(target, enabled, broadcasts)
            }
            ControlCommand::AgentWait { .. } => unreachable!("agent waits are registered above"),
            ControlCommand::AgentPrompt { .. } => {
                unreachable!("agent prompts are submitted above")
            }
            ControlCommand::AgentReport {
                target,
                agent,
                integration,
                state,
                reason,
                native_session,
                seq,
            } => self.session_agent_report_update(
                target,
                Some(agent),
                integration,
                Some((state, reason, native_session)),
                seq,
                broadcasts,
            ),
            ControlCommand::AgentRelease {
                target,
                integration,
                seq,
            } => self.session_agent_report_update(target, None, integration, None, seq, broadcasts),
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
            .map(|(id, pane)| {
                let reference = protocol::PaneRef {
                    session_instance: self.instance_id.clone(),
                    pane_id: *id,
                    generation: pane.generation,
                };
                let references = pane.agent.references(reference.clone());
                let runtime = protocol::effective_agent_runtimes(&pane.runtime, &references)
                    .into_iter()
                    .find(|runtime| runtime.reference.slot.is_none());
                SessionPaneInfo {
                    session: self.session_name.clone(),
                    id: *id,
                    reference,
                    agent_ref: runtime.as_ref().map(|runtime| runtime.reference.clone()),
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
                    agent: runtime.as_ref().map(|runtime| runtime.identity.id.clone()),
                    agent_state: runtime
                        .as_ref()
                        .map(|runtime| runtime.state.as_str().to_string()),
                }
            })
            .collect();
        // A HashMap iteration order would reshuffle the table between two identical calls.
        panes.sort_by_key(|pane| pane.id);
        panes
    }

    fn session_agent_report(&self) -> Vec<AgentInfo> {
        let workspaces = self.layout_workspace_index();
        let mut agents = Vec::new();
        for (&pane_id, pane) in &self.panes {
            if pane.exited.is_some() {
                continue;
            }
            for runtime in self.agent_runtimes_for(pane_id, pane) {
                agents.push(AgentInfo {
                    session: self.session_name.clone(),
                    pane: pane_id,
                    workspace: workspaces.get(&pane_id).copied().unwrap_or(0),
                    agent: runtime.identity.id,
                    label: runtime.label,
                    state: runtime.state,
                    reason: runtime.reason,
                    cwd: pane.runtime.cwd.clone(),
                    native_session: pane
                        .runtime
                        .integration
                        .as_ref()
                        .and_then(|report| report.native_session.clone()),
                    reference: runtime.reference,
                    source: runtime.source,
                });
            }
        }
        agents.sort_by(|left, right| {
            left.pane
                .cmp(&right.pane)
                .then_with(|| left.reference.slot.cmp(&right.reference.slot))
        });
        agents
    }

    fn session_agent_get(&self, target: AgentTarget) -> ControlResponse {
        let reference = match self.resolve_agent_wait_target(target) {
            Ok(reference) => reference,
            Err(response) => return response,
        };
        self.session_agent_report()
            .into_iter()
            .find(|agent| agent.reference == reference)
            .map(ControlResponse::ok)
            .unwrap_or_else(|| {
                ControlResponse::error_with(
                    ControlErrorCode::AgentGone,
                    "agent is no longer present",
                )
            })
    }

    fn register_agent_prompt(
        &mut self,
        client_id: ClientId,
        request: SessionAgentPrompt<'_>,
        capabilities: protocol::Capabilities,
        effective_protocol: u32,
    ) -> Option<ControlResponse> {
        if request.prompt.is_empty() {
            return Some(ControlResponse::error_with(
                ControlErrorCode::InvalidArgument,
                "agent prompt cannot be empty",
            ));
        }
        let reference = match self.resolve_agent_wait_target(request.target) {
            Ok(reference) => reference,
            Err(response) => return Some(response),
        };
        if let Err(response) = self.validate_agent_input_reference(&reference) {
            return Some(response);
        }
        let Some(pane) = self.panes.get(&reference.pane.pane_id) else {
            return Some(ControlResponse::error_with(
                ControlErrorCode::AgentGone,
                "agent pane is gone",
            ));
        };
        let Some(runtime) = self
            .agent_runtimes_for(reference.pane.pane_id, pane)
            .into_iter()
            .find(|runtime| runtime.reference == reference)
        else {
            return Some(ControlResponse::error_with(
                ControlErrorCode::AgentReplaced,
                "agent incarnation was replaced",
            ));
        };
        if let Err(response) = validate_agent_prompt_state(runtime.state, request.allow_working) {
            return Some(response);
        }

        if let Some(until) = request.wait
            && let Err(response) = self.register_post_prompt_wait(
                client_id,
                reference.clone(),
                until,
                request.timeout_ms,
                capabilities,
                effective_protocol,
            )
        {
            return Some(response);
        }

        let mut bytes = request.prompt.as_bytes().to_vec();
        bytes.push(b'\r');
        let response = self.session_send_bytes(Some(reference.pane.pane_id), bytes);
        if !response.ok {
            self.remove_agent_wait(client_id);
            return Some(response);
        }
        if request.wait.is_some() {
            None
        } else {
            Some(ControlResponse::ok(serde_json::json!({
                "accepted": true,
                "ref": reference,
            })))
        }
    }

    fn validate_agent_input_reference(
        &self,
        reference: &protocol::AgentRef,
    ) -> std::result::Result<(), ControlResponse> {
        let Some(slot) = reference.slot.as_deref() else {
            return Ok(());
        };
        let active = self
            .panes
            .get(&reference.pane.pane_id)
            .and_then(|pane| pane.runtime.rows.iter().find(|row| row.id == slot))
            .is_some_and(|row| row.active);
        if active {
            Ok(())
        } else {
            Err(ControlResponse::error_with(
                ControlErrorCode::Conflict,
                format!(
                    "published agent slot `{slot}` is not active; read and prompt only target the visible activity"
                ),
            ))
        }
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
                Err(ControlResponse::error_with(
                    ControlErrorCode::PaneNotFound,
                    format!("pane {id} not found"),
                ))
            };
        }
        let mut ids: Vec<PaneId> = self.panes.keys().copied().collect();
        ids.sort_unstable();
        match ids.as_slice() {
            [only] => Ok(*only),
            [] => Err(ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("session `{}` has no panes", self.session_name),
            )),
            many => Err(ControlResponse::error_with(
                ControlErrorCode::TargetRequired,
                format!(
                    "session `{}` has {} panes and no focused pane; pass --target (ids: {})",
                    self.session_name,
                    many.len(),
                    many.iter()
                        .map(PaneId::to_string)
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
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
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
        };
        // Reading a snapshot does not change what a replay would contain, so this must not bump
        // `content_generation` and make every snapshot re-export the pane it just captured.
        let text = match crate::pane::capture_screen_text(pane.screen_without_change(), scrollback)
        {
            Ok(text) => text,
            Err(error) => return ControlResponse::error(error),
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
            return ControlResponse::error_with(
                ControlErrorCode::InputLocked,
                format!(
                    "session `{}` has input locked; unlock it from the attached client",
                    self.session_name
                ),
            );
        }
        let Some(pane) = self.panes.get(&id) else {
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
        };
        if pane.exited.is_some() || pane.pty.is_none() {
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotRunning,
                format!("pane {id} PTY is not running"),
            );
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
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
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
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
        };
        match self.apply_pane_status(None, id, generation, status, reason) {
            Ok(Some(state)) => {
                broadcasts.push((
                    Target::Broadcast,
                    ServerMessage::PaneRuntimeChanged {
                        pane_id: id,
                        local: false,
                        generation,
                        agent_refs: self.agent_references(None, id),
                        state,
                    },
                ));
                ControlResponse::empty()
            }
            Ok(None) => ControlResponse::empty(),
            Err((_, message)) => ControlResponse::error(message),
        }
    }

    fn session_agent_report_update(
        &mut self,
        target: Option<PaneId>,
        agent: Option<String>,
        integration: String,
        report: Option<(protocol::AgentState, Option<String>, Option<String>)>,
        seq: u64,
        broadcasts: &mut Vec<(Target, ServerMessage)>,
    ) -> ControlResponse {
        let id = match self.session_target_pane(target) {
            Ok(id) => id,
            Err(response) => return response,
        };
        let Some(generation) = self.panes.get(&id).map(|pane| pane.generation) else {
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
        };
        let (state, reason, native_session) = report.map_or((None, None, None), |report| {
            (Some(report.0), report.1, report.2)
        });
        match self.apply_agent_integration(
            None,
            id,
            generation,
            runtime::AgentIntegrationUpdate {
                agent,
                integration,
                state,
                reason,
                native_session,
                seq,
            },
        ) {
            Ok(Some(state)) => {
                broadcasts.push((
                    Target::Broadcast,
                    ServerMessage::PaneRuntimeChanged {
                        pane_id: id,
                        local: false,
                        generation,
                        agent_refs: self.agent_references(None, id),
                        state,
                    },
                ));
                ControlResponse::empty()
            }
            Ok(None) => ControlResponse::empty(),
            Err((code, message)) => {
                ControlResponse::error_with(runtime::integration_error_code(code), message)
            }
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
            return ControlResponse::error_with(
                ControlErrorCode::PaneNotFound,
                format!("pane {id} not found"),
            );
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
        // Opening a shared pane means committing a layout revision, and the lease says who may do
        // that. A client holding it is arranging this session right now; a revision arriving from
        // somewhere else would reflow its screen and race its next commit. The same rule the
        // protocol already applies to `SpawnPane` from a non-controller applies here, and it is
        // what keeps "no new authority" true: nothing reaches this server headlessly that an
        // attached client could not have sent.
        if let Some(controller) = self.controller {
            return ControlResponse::error(format!(
                "client {controller} holds layout control of session `{}`; ask it to open the pane, or detach it first",
                self.session_name
            ));
        }
        // `[[rules]]`, the shell, and the command runner used here were re-read from config just
        // before this ran - see the `SessionControl` arm in `connection.rs`, which does it for
        // every request that opens a pane. They describe the pane about to exist, not the ones
        // this server opened when it started.
        let launch = match crate::pane::spawn_policy::requested_launch(spawn.command, spawn.argv) {
            Ok(launch) => launch,
            Err(error) => return ControlResponse::error(error),
        };
        let requested_workspace = match spawn
            .workspace
            .map(crate::pane::spawn_policy::workspace_index)
        {
            Some(Ok(index)) => Some(index),
            Some(Err(error)) => return ControlResponse::error(error),
            None => None,
        };
        // `[[rules]]` decide float, fullscreen, and - unless the caller named one - the workspace,
        // exactly as they do for a pane a person opens. `focus` is resolved and then ignored:
        // there is no focus here, which is why `--focus` was refused above.
        let rule_command = launch
            .as_ref()
            .map(crate::pane::launch::PaneLaunch::display);
        let (placement_workspace, placement) = crate::pane::spawn_policy::resolve_placement(
            &self.settings.rules,
            rule_command.as_deref(),
            requested_workspace,
            Some(false),
        );
        let workspace_index = placement_workspace.unwrap_or(HEADLESS_DEFAULT_WORKSPACE);

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
        // `spawn_pane` raises `next_generation` past whatever it is handed, so nothing is consumed
        // by a request that turns out to be refused below.
        let generation = self.next_generation;

        // Build and validate the document *before* the PTY exists. A layout this server would
        // itself reject leaves the old revision standing, and a pane committed to nothing is a
        // process no client can see and nobody asked to keep.
        let Some(layout) = self.headless_layout_with_pane(
            pane_id,
            generation,
            workspace_index,
            &placement,
            launch.clone(),
            spawn.cwd.clone(),
            spawn.title.clone(),
            spawn.keep_open,
        ) else {
            return ControlResponse::error(format!(
                "placing pane {pane_id} in workspace {} would produce an invalid layout",
                workspace_index + 1
            ));
        };

        let (cols, rows) = self.headless_spawn_size();
        let palette = self
            .panes
            .values()
            .next()
            .map(|pane| pane.palette)
            .unwrap_or_else(|| WirePalette::from(TerminalColorPalette::default()));
        let shell = self.settings.shell.clone();
        let command_shell = self.settings.command_shell.clone();
        let result = self.spawn_pane(SpawnRequest {
            pane_id,
            owner: None,
            generation,
            launch,
            cwd: spawn.cwd,
            title: spawn.title,
            cols,
            rows,
            keep_open: spawn.keep_open,
            env: crate::pane::spawn_policy::spawn_environment(
                crate::pane::spawn_policy::SpawnOrigin::Headless,
                pane_id,
                &[],
            ),
            palette,
            shell,
            command_shell,
            cell: None,
        });
        // `SpawnResult` answers the client that asked; a follower learns about a new pane from
        // the layout revision below, exactly as it does for a controller's split. Broadcasting it
        // would hand every client a reply to a request it never made.
        let pty_ready = match result {
            ServerMessage::SpawnResult { ok: true, .. } => true,
            ServerMessage::SpawnResult { error, .. } => {
                return ControlResponse::error(error.unwrap_or_else(|| "spawn failed".to_string()));
            }
            _ => false,
        };

        self.layout_rev += 1;
        self.layout = Some(layout.clone());
        self.mark_dirty();
        broadcasts.push((
            Target::Broadcast,
            ServerMessage::LayoutCommitted {
                rev: self.layout_rev,
                // Client ids start at 1, so `0` is a document no client authored. Every
                // client therefore reconciles this revision instead of recognising it as the
                // echo of its own commit - which is right: none of them wrote it. Nobody holds
                // the lease while this runs; the gate at the top of this function saw to that.
                author: SERVER_LAYOUT_AUTHOR,
                layout,
            },
        ));

        ControlResponse::ok(SessionNewPane {
            id: pane_id,
            accepted: true,
            pty_ready,
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

    /// The shared layout with the new pane appended to `workspace_index`, or `None` when the
    /// result would not validate.
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
        placement: &crate::pane::lifecycle::SpawnPlacement,
        launch: Option<crate::pane::launch::PaneLaunch>,
        cwd: Option<String>,
        title: Option<String>,
        keep_open: bool,
    ) -> Option<SharedLayout> {
        let mut layout = self.layout.clone().unwrap_or_else(|| SharedLayout {
            version: SHARED_LAYOUT_VERSION,
            canvas_cols: HEADLESS_SPAWN_COLS,
            canvas_rows: HEADLESS_SPAWN_ROWS,
            workspaces: Vec::new(),
        });
        let shared_pane = SharedPane {
            pane_id,
            generation,
            title,
            profile_name: None,
            cwd,
            launch,
            replay: false,
            keep_open,
            floating: placement.float.is_some(),
            fullscreen: placement.fullscreen,
            // A floating pane must carry a rect, and `SpawnFloat::rect` is the one implementation
            // of where a float lands. It works in canvas cells, so it is run against the canvas
            // this document declares and divided back into the fractions the document stores -
            // the same round trip a client's own float makes on its way onto the wire.
            rect: placement.float.map(|float| {
                float_rect_to_frac(
                    float.rect(FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: f32::from(layout.canvas_cols.max(1)),
                        h: f32::from(layout.canvas_rows.max(1)),
                    }),
                    layout.canvas_cols,
                    layout.canvas_rows,
                )
            }),
            scrollable_width: crate::state::DEFAULT_SCROLLABLE_WIDTH,
        };
        match layout
            .workspaces
            .iter_mut()
            .find(|workspace| workspace.index == workspace_index)
        {
            Some(workspace) => {
                // At most one pane per workspace is fullscreen: two would stack, and which one a
                // client drew would come down to order. Same rule `spawn_pane_in_workspace` keeps.
                if placement.fullscreen {
                    for other in &mut workspace.panes {
                        other.fullscreen = false;
                    }
                }
                workspace.panes.push(shared_pane);
            }
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
        // A document the server would itself reject is worse than no document: the caller is told
        // so before a PTY exists, and the revision every attached client holds is left standing.
        layout.validate().ok()?;
        Some(layout)
    }
}

fn validate_agent_prompt_state(
    state: protocol::AgentState,
    allow_working: bool,
) -> std::result::Result<(), ControlResponse> {
    match state {
        protocol::AgentState::Blocked => Err(ControlResponse::error_with(
            ControlErrorCode::AgentBlocked,
            "agent is blocked; refusing to type into a prompt or approval dialog",
        )),
        protocol::AgentState::Working if !allow_working => Err(ControlResponse::error_with(
            ControlErrorCode::Conflict,
            "agent is working; pass --allow-working to submit anyway",
        )),
        _ => Ok(()),
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
            1,
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

    fn pane_with_agent(server: &mut SessionServer, id: PaneId) {
        let mut pane = super::super::tests::test_pane(1);
        pane.runtime.cwd = Some("/repo".into());
        pane.runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Idle,
        });
        pane.agent.sync_references(&pane.runtime);
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
            1,
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
            1,
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
    fn agent_list_and_get_return_the_same_exact_reference() {
        let mut server = SessionServer::new_named("dev");
        pane_with_agent(&mut server, 7);

        let (listed, _) = control(&mut server, ControlCommand::AgentsList);
        let agents: Vec<AgentInfo> = serde_json::from_value(listed.data.unwrap()).unwrap();
        assert_eq!(agents.len(), 1);
        assert_eq!(agents[0].pane, 7);
        assert_eq!(agents[0].state, protocol::AgentState::Idle);

        let (got, _) = control(
            &mut server,
            ControlCommand::AgentGet {
                target: AgentTarget::Ref(agents[0].reference.clone()),
            },
        );
        let got: AgentInfo = serde_json::from_value(got.data.unwrap()).unwrap();
        assert_eq!(got, agents[0]);
    }

    #[test]
    fn published_slot_input_requires_the_active_activity() {
        let mut server = SessionServer::new_named("dev");
        pane_with_agent(&mut server, 7);
        let pane = server.panes.get_mut(&7).unwrap();
        pane.runtime.rows = vec![
            protocol::PublishedRow {
                id: "visible".into(),
                title: "Visible".into(),
                status: "idle".into(),
                reason: None,
                active: true,
                work_started_at: None,
            },
            protocol::PublishedRow {
                id: "hidden".into(),
                title: "Hidden".into(),
                status: "working".into(),
                reason: None,
                active: false,
                work_started_at: None,
            },
        ];
        pane.agent.sync_references(&pane.runtime);
        let agents = server.session_agent_report();
        let visible = &agents
            .iter()
            .find(|agent| agent.reference.slot.as_deref() == Some("visible"))
            .unwrap()
            .reference;
        let hidden = &agents
            .iter()
            .find(|agent| agent.reference.slot.as_deref() == Some("hidden"))
            .unwrap()
            .reference;

        assert!(server.validate_agent_input_reference(visible).is_ok());
        assert_eq!(
            server
                .validate_agent_input_reference(hidden)
                .unwrap_err()
                .code,
            Some(ControlErrorCode::Conflict)
        );
        assert!(
            server
                .session_agent_get(AgentTarget::Ref(hidden.clone()))
                .ok
        );
    }

    #[test]
    fn atomic_prompt_refuses_a_blocked_agent_before_writing() {
        let mut server = SessionServer::new_named("dev");
        pane_with_agent(&mut server, 7);
        server.panes.get_mut(&7).unwrap().runtime.detected_agent = Some(protocol::DetectedAgent {
            agent: protocol::AgentIdentity::new("claude", "Claude Code").into(),
            state: protocol::DetectedAgentState::Blocked,
        });
        let response = server
            .register_agent_prompt(
                1,
                SessionAgentPrompt {
                    target: AgentTarget::Pane(7),
                    prompt: "approve this",
                    wait: None,
                    timeout_ms: None,
                    allow_working: false,
                },
                protocol::Capabilities::default(),
                PROTOCOL_VERSION,
            )
            .unwrap();
        assert_eq!(response.code, Some(ControlErrorCode::AgentBlocked));
        assert!(server.agent_waits.is_empty());
    }

    #[test]
    fn integration_reports_and_releases_are_sequence_fenced() {
        let mut server = SessionServer::new_named("dev");
        pane_with_agent(&mut server, 7);
        {
            let pane = server.panes.get_mut(&7).unwrap();
            pane.runtime.rows.push(protocol::PublishedRow {
                id: "old-session".into(),
                title: "Old session".into(),
                status: "working".into(),
                reason: None,
                active: true,
                work_started_at: None,
            });
            pane.agent.sync_references(&pane.runtime);
        }
        let (accepted, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-a".into(),
                state: protocol::AgentState::Idle,
                reason: Some("ready".into()),
                native_session: Some("native-123".into()),
                seq: 13,
            },
        );
        assert!(accepted.ok);
        let bound = server.panes[&7].runtime.integration.as_ref().unwrap();
        assert_eq!(bound.identity.id, "claude");
        assert!(bound.reference.incarnation > 0);
        assert_eq!(bound.seq, 13);
        assert!(server.panes[&7].runtime.rows.is_empty());

        let (stale, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-a".into(),
                state: protocol::AgentState::Blocked,
                reason: None,
                native_session: None,
                seq: 12,
            },
        );
        assert_eq!(stale.code, Some(ControlErrorCode::Conflict));
        assert_eq!(
            server.panes[&7].runtime.integration.as_ref().unwrap().state,
            protocol::AgentState::Idle
        );

        assert!(
            control(
                &mut server,
                ControlCommand::AgentRelease {
                    target: Some(7),
                    integration: "hook-a".into(),
                    seq: 14,
                },
            )
            .0
            .ok
        );
        assert!(server.panes[&7].runtime.integration.is_none());

        let (next, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-b".into(),
                state: protocol::AgentState::Working,
                reason: None,
                native_session: None,
                seq: 1,
            },
        );
        assert!(next.ok, "{next:?}");
        let started = server.panes[&7]
            .runtime
            .work_started_at
            .expect("working integration starts the run clock");
        server.panes.get_mut(&7).unwrap().agent.summary_changed_at = 11;
        let (heartbeat, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-b".into(),
                state: protocol::AgentState::Working,
                reason: Some("still working".into()),
                native_session: None,
                seq: 2,
            },
        );
        assert!(heartbeat.ok, "{heartbeat:?}");
        assert_eq!(server.panes[&7].agent.summary_changed_at, 11);
        assert_eq!(server.panes[&7].runtime.work_started_at, Some(started));

        server.panes.get_mut(&7).unwrap().agent.summary_changed_at = 0;
        let (blocked, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-b".into(),
                state: protocol::AgentState::Blocked,
                reason: Some("x".repeat(protocol::PANE_STATUS_REASON_MAX_LEN + 10)),
                native_session: None,
                seq: 3,
            },
        );
        assert!(blocked.ok, "{blocked:?}");
        assert_eq!(server.panes[&7].runtime.work_started_at, Some(started));
        assert_eq!(
            server.panes[&7]
                .runtime
                .integration
                .as_ref()
                .unwrap()
                .reason
                .as_ref()
                .unwrap()
                .chars()
                .count(),
            protocol::PANE_STATUS_REASON_MAX_LEN
        );
        assert!(server.panes[&7].agent.summary_changed_at > 0);

        let (delayed, _) = control(
            &mut server,
            ControlCommand::AgentReport {
                target: Some(7),
                agent: "claude".into(),
                integration: "hook-a".into(),
                state: protocol::AgentState::Working,
                reason: None,
                native_session: None,
                seq: 15,
            },
        );
        assert_eq!(delayed.code, Some(ControlErrorCode::Conflict));
        assert_eq!(
            server.panes[&7]
                .runtime
                .integration
                .as_ref()
                .unwrap()
                .integration,
            "hook-b"
        );
        assert!(
            control(
                &mut server,
                ControlCommand::AgentRelease {
                    target: Some(7),
                    integration: "hook-b".into(),
                    seq: 4,
                },
            )
            .0
            .ok
        );
        assert_eq!(server.panes[&7].runtime.work_started_at, None);
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

    /// Committing a layout revision is the controller's job. A headless caller is not the
    /// controller and cannot become one, so while a client holds the lease this is refused rather
    /// than racing that client's next commit and reflowing the screen it is working in.
    #[test]
    fn a_headless_spawn_is_refused_while_a_client_holds_layout_control() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 1, b"");
        server.layout = Some(one_pane_layout(1));
        let before_rev = server.layout_rev;
        server.controller = Some(7);

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
        let error = response.error.unwrap_or_default();
        assert!(error.contains("layout control"), "{error}");
        assert!(broadcasts.is_empty(), "a refused spawn changes nothing");
        assert_eq!(server.panes.len(), 1, "and leaves no pane behind");
        assert_eq!(server.layout_rev, before_rev);

        // The lease moving away is all it takes; nothing else about the request changed.
        server.controller = None;
        let (allowed, _) = control(
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
        assert!(allowed.ok, "{:?}", allowed.error);
    }

    /// A pane id is not a session. `ROZI_PANE` names pane 3 of whatever session the caller is
    /// sitting in, and `--session` says the request is for a different one, where pane 3 - if it
    /// exists at all - is a stranger. Honouring it would also disarm the ambiguity check, since an
    /// inherited id is indistinguishable from an explicit `--target`.
    #[test]
    fn an_inherited_pane_id_never_addresses_a_pane_in_the_session_being_targeted() {
        let mut server = SessionServer::new_named("dev");
        for id in [3, 4] {
            pane_with_screen(&mut server, id, b"");
        }

        // The caller is inside pane 3 of some *other* session. `dev` has a pane 3 too.
        let messages = server.handle_session_control(
            1,
            "dev".to_string(),
            PROTOCOL_VERSION,
            protocol::MIN_SUPPORTED_PROTOCOL,
            None,
            ControlRequest {
                command: ControlCommand::SendText {
                    target: None,
                    text: "cargo test\n".to_string(),
                },
                source_pane: Some(3),
                extension: None,
            },
        );
        let [(Target::Sender, ServerMessage::SessionControlResult { response, .. })] =
            messages.as_slice()
        else {
            panic!("expected one control result, got {messages:?}");
        };
        assert!(
            !response.ok,
            "an inherited pane id must not stand in for a target"
        );
        let error = response.error.clone().unwrap_or_default();
        assert!(error.contains("--target"), "{error}");
        assert!(
            error.contains("3, 4"),
            "the ambiguity check still runs: {error}"
        );

        // Naming the pane explicitly is how a caller addresses one.
        let (targeted, _) = control(
            &mut server,
            ControlCommand::SendText {
                target: Some(3),
                text: "ok".to_string(),
            },
        );
        // Pane 3 has no PTY in this fixture, so this reaches the liveness check rather than the
        // ambiguity one - which is the point: the target resolved.
        assert!(
            targeted
                .error
                .unwrap_or_default()
                .contains("PTY is not running"),
            "an explicit target resolves"
        );
    }

    /// Spawn policy describes the next pane, so it is re-read rather than frozen at startup. The
    /// read itself belongs to the socket path; this covers the swap it performs.
    #[test]
    fn refreshed_spawn_policy_replaces_what_the_server_started_with() {
        let mut server = SessionServer::new_named("dev");
        assert!(server.settings.rules.is_empty());

        let rule = crate::config::RuleConfig {
            matcher: crate::config::RuleMatcher::Substring("btop".to_string()),
            float: false,
            width: None,
            height: None,
            workspace: Some(5),
            focus: true,
            fullscreen: false,
            position: crate::config::FloatPosition::Center,
        };
        server.apply_spawn_policy(
            vec![rule.clone()],
            vec!["/bin/dash".to_string()],
            vec!["/bin/dash".to_string(), "-c".to_string()],
        );
        assert_eq!(server.settings.rules, vec![rule]);
        assert_eq!(server.settings.shell, vec!["/bin/dash".to_string()]);

        // An edit that removes every rule has to take effect too, not just one that adds them.
        server.apply_spawn_policy(Vec::new(), Vec::new(), Vec::new());
        assert!(server.settings.rules.is_empty());
    }

    /// Reading and typing are not layout changes, so the lease does not gate them - a follower
    /// client may already do both. Only the commit is controller-only.
    #[test]
    fn a_controller_does_not_stop_headless_reads() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 1, b"hello\r\n");
        server.controller = Some(7);

        let (listed, _) = control(&mut server, ControlCommand::ListPanes);
        assert!(listed.ok);
        let (captured, _) = control(
            &mut server,
            ControlCommand::CapturePane {
                target: Some(1),
                scrollback: None,
            },
        );
        assert!(captured.ok, "{:?}", captured.error);
    }

    /// The generation on an extension's request is a fencing token only the client that minted it
    /// can check. A server that cannot check it must not act on it, or `--session` becomes the way
    /// around a fence the UI endpoint enforces.
    #[test]
    fn a_request_carrying_extension_provenance_is_refused_rather_than_trusted() {
        let mut server = SessionServer::new_named("dev");
        pane_with_screen(&mut server, 1, b"");

        for command in [
            ControlCommand::ListPanes,
            ControlCommand::SendText {
                target: Some(1),
                text: "rm -rf /\n".to_string(),
            },
        ] {
            let messages = server.handle_session_control(
                1,
                "dev".to_string(),
                PROTOCOL_VERSION,
                protocol::MIN_SUPPORTED_PROTOCOL,
                None,
                ControlRequest {
                    command,
                    source_pane: None,
                    extension: Some(crate::config::ExtensionProvenance {
                        id: "git-tools".to_string(),
                        generation: "whatever-the-caller-claims".to_string(),
                    }),
                },
            );
            let [(Target::Sender, ServerMessage::SessionControlResult { response, .. })] =
                messages.as_slice()
            else {
                panic!("expected exactly one control result, got {messages:?}");
            };
            assert!(!response.ok);
            let error = response.error.clone().unwrap_or_default();
            assert!(error.contains("git-tools"), "{error}");
            assert!(error.contains("cannot check"), "{error}");
        }
    }

    /// `[[rules]]` are config, not UI: the same entry that floats a command for a keypress floats
    /// it for a script. The server holds its own copy precisely because no client is there to
    /// apply one.
    #[test]
    fn a_headless_spawn_obeys_the_same_rules_a_client_spawn_would() {
        let mut server = SessionServer::new_named("dev");
        server.settings.rules = vec![crate::config::RuleConfig {
            matcher: crate::config::RuleMatcher::Substring("btop".to_string()),
            float: true,
            width: Some(0.5),
            height: Some(0.4),
            workspace: Some(2),
            focus: true,
            fullscreen: false,
            position: crate::config::FloatPosition::Center,
        }];

        let (response, broadcasts) = control(
            &mut server,
            ControlCommand::NewPane {
                command: Some("btop".to_string()),
                argv: None,
                cwd: None,
                title: None,
                keep_open: false,
                focus: false,
                workspace: None,
            },
        );
        assert!(response.ok, "{:?}", response.error);
        let [(_, ServerMessage::LayoutCommitted { layout, .. })] = broadcasts.as_slice() else {
            panic!("expected a layout commit, got {broadcasts:?}");
        };
        let workspace = layout
            .workspaces
            .iter()
            .find(|workspace| workspace.index == 2)
            .expect("the rule's workspace, not the headless default");
        let pane = &workspace.panes[0];
        assert!(pane.floating, "the rule floats this command");
        let rect = pane.rect.expect("a floating pane carries a rect");
        // Fractions of the document's canvas, centred, at the rule's size.
        assert!((rect.w - 0.5).abs() < 0.02, "{rect:?}");
        assert!((rect.h - 0.4).abs() < 0.02, "{rect:?}");
        assert!((rect.x - 0.25).abs() < 0.02, "{rect:?}");
        layout.validate().expect("a ruled float still validates");
    }

    /// The PTY is the expensive, visible half of a spawn. A layout that would not validate is
    /// found before one exists, so a refused request leaves no orphan process behind.
    #[test]
    fn a_layout_that_would_not_validate_is_refused_before_any_pty_is_started() {
        let mut server = SessionServer::new_named("dev");
        // A document from a future build: the server can hold it, and must not extend it.
        server.layout = Some(SharedLayout {
            version: SHARED_LAYOUT_VERSION + 1,
            canvas_cols: 80,
            canvas_rows: 24,
            workspaces: Vec::new(),
        });
        let before_rev = server.layout_rev;

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
                .contains("invalid layout"),
            "the refusal must name what went wrong"
        );
        assert!(server.panes.is_empty(), "no pane, and therefore no PTY");
        assert!(broadcasts.is_empty());
        assert_eq!(server.layout_rev, before_rev);
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
                empty: None,
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
