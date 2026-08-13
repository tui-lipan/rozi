use super::pane_log::LogHeader;
use super::*;
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

impl SessionServer {
    /// Rename this session in place: start listening on the new name's endpoint so the same server
    /// (and its live panes) becomes discoverable under `name` with zero pane movement. Rejects
    /// invalid names and collisions with an already-running session.
    ///
    /// The new endpoint is *bound before* the old listener is retired, so the session is never
    /// momentarily discoverable under neither name; the accept loop swaps the listener in on its
    /// next pass (see [`SessionServer::run_listener`]) and drops the old one then. Already-accepted
    /// clients are untouched by either step and stay attached across the rename.
    pub(super) fn rename_session(&mut self, name: String) -> ServerMessage {
        if !crate::session::discovery::valid_session_name(&name) {
            return ServerMessage::Error {
                code: "invalid-name".to_string(),
                message: format!("invalid session name {name:?}"),
            };
        }
        if name == self.session_name {
            return ServerMessage::Renamed { session: name };
        }
        let new_endpoint = match session_endpoint(&name) {
            Ok(endpoint) => endpoint,
            Err(err) => {
                return ServerMessage::Error {
                    code: "rename-failed".to_string(),
                    message: err.to_string(),
                };
            }
        };
        if new_endpoint.is_live() {
            return ServerMessage::Error {
                code: "name-in-use".to_string(),
                message: format!("session `{name}` already exists"),
            };
        }
        let bound = match new_endpoint.bind() {
            Ok(bound) => bound,
            Err(err) => {
                return ServerMessage::Error {
                    code: "rename-failed".to_string(),
                    message: err.to_string(),
                };
            }
        };
        let listener = bound.into_listener();
        if let Err(err) = listener.set_nonblocking(true) {
            return ServerMessage::Error {
                code: "rename-failed".to_string(),
                message: err.to_string(),
            };
        }
        let retired = self.endpoint.replace(new_endpoint);
        self.pending_listener = Some((listener, retired));
        self.session_name = name.clone();
        ServerMessage::Renamed { session: name }
    }

    pub(super) fn spawn_pane(&mut self, request: SpawnRequest) -> ServerMessage {
        self.spawn_pane_inner(request, None, false)
    }

    pub(super) fn spawn_pane_inner(
        &mut self,
        request: SpawnRequest,
        seed: Option<&[u8]>,
        retain_on_failure: bool,
    ) -> ServerMessage {
        let id = request.pane_id;
        let owner = request.owner;
        // A live pane with this id already exists; refuse. An *exited* pane is replaced in place
        // so keep-open respawn (client re-sends `SpawnPane` with a fresh generation) works.
        if self
            .pane(owner, id)
            .is_some_and(|pane| pane.exited.is_none())
        {
            return ServerMessage::SpawnResult {
                pane_id: id,
                local: wire_local(owner),
                generation: request.generation,
                pid: None,
                ok: false,
                error: Some(format!("pane {id} already exists")),
            };
        }
        // A replaced pane's log handle dies with it; tell every client so no stale
        // `[log]` badge survives a respawn.
        if let Some(old) = self.remove_owned_pane(owner, id)
            && old.log.is_some()
        {
            self.send_outbound(
                owner,
                &ServerOutbound::control(ServerMessage::PaneLoggingChanged {
                    pane_id: id,
                    local: wire_local(owner),
                    generation: old.generation,
                    enabled: false,
                    path: None,
                    error: None,
                }),
            );
        }
        let cols = if request.cols == 0 {
            DEFAULT_COLS
        } else {
            request.cols
        };
        let rows = if request.rows == 0 {
            DEFAULT_ROWS
        } else {
            request.rows
        };
        let generation = request.generation;
        self.next_generation = self.next_generation.max(generation.saturating_add(1));
        // Pre-17 clients report no cell size; the PTY's own default stands in until one resizes.
        let cell = request.cell.unwrap_or_default();
        let mut screen = TerminalScreen::new(rows.max(1), cols.max(1), self.settings.scrollback);
        screen.set_cell_size(cell);
        // Seed the palette before the PTY spawns so the child's startup OSC 4/10/11 color queries
        // are answered against the theme palette instead of the screen default.
        screen.set_palette(request.palette.into());
        if let Some(seed) = seed {
            screen.process_bytes(seed);
            screen.drain_responses();
            screen.drain_clipboard_events();
        }
        let mut config = pty_config(
            request.command.as_deref(),
            &request.shell,
            &request.command_shell,
        )
        .size(cols.max(1), rows.max(1))
        .cell_size(cell);
        let effective_cwd = effective_spawn_cwd(request.cwd.as_deref());
        if let Some(cwd) = effective_cwd.as_ref() {
            config = config.cwd(cwd.clone());
        }
        for (key, value) in &request.env {
            config = config.env(key.clone(), value.clone());
        }
        let events = Arc::clone(&self.events);
        match TerminalPty::spawn(config, move |event| {
            let event = ServerEvent::Pty(owner, id, generation, event);
            let bytes = event.payload_bytes();
            let _ = events.push_blocking_with(event, bytes, ServerEvent::coalesce_output);
        }) {
            Ok(pty) => {
                let pid = pty.pid();
                screen.resize(rows.max(1), cols.max(1));
                self.insert_owned_pane(
                    owner,
                    id,
                    ServerPane {
                        generation,
                        title: request.title,
                        cwd: effective_cwd,
                        command: request.command,
                        keep_open: request.keep_open,
                        command_completed: false,
                        palette: request.palette,
                        pty: Some(pty),
                        terminal: screen,
                        content_generation: 0,
                        cols: cols.max(1),
                        rows: rows.max(1),
                        cell,
                        exited: None,
                        log: None,
                        shell: request.shell,
                        env: request.env,
                        runtime: protocol::PaneRuntimeState::default(),
                        agent: AgentScratch::default(),
                        last_git_read: None,
                        initial_cursor_report_primed: cfg!(windows),
                    },
                );
                if owner.is_none() {
                    self.mark_dirty();
                }
                self.sync_pane_runtime(owner, id, generation);
                ServerMessage::SpawnResult {
                    pane_id: id,
                    local: wire_local(owner),
                    generation,
                    pid,
                    ok: true,
                    error: None,
                }
            }
            Err(err) => {
                if retain_on_failure {
                    self.insert_owned_pane(
                        owner,
                        id,
                        ServerPane {
                            generation,
                            title: request.title,
                            cwd: request.cwd,
                            command: request.command,
                            keep_open: request.keep_open,
                            command_completed: false,
                            palette: request.palette,
                            pty: None,
                            terminal: screen,
                            content_generation: 0,
                            cols: cols.max(1),
                            rows: rows.max(1),
                            cell,
                            exited: Some(127),
                            log: None,
                            shell: request.shell,
                            env: request.env,
                            runtime: protocol::PaneRuntimeState::default(),
                            agent: AgentScratch::default(),
                            last_git_read: None,
                            initial_cursor_report_primed: false,
                        },
                    );
                }
                ServerMessage::SpawnResult {
                    pane_id: id,
                    local: wire_local(owner),
                    generation,
                    pid: None,
                    ok: false,
                    error: Some(err.to_string()),
                }
            }
        }
    }

    pub(super) fn handle_pane_input(
        &mut self,
        owner: Option<ClientId>,
        pane_id: PaneId,
        generation: u64,
        bytes: &[u8],
    ) {
        if let Some(pane) = self.live_pane_mut(owner, pane_id, generation)
            && let Some(pty) = &pane.pty
        {
            let _ = pty.write(bytes);
        }
    }

    pub(super) fn handle_event(&mut self, event: ServerEvent) -> Option<ServerOutbound> {
        match event {
            ServerEvent::Pty(owner, id, generation, event) => {
                let pane = self.pane_mut(owner, id)?;
                if pane.generation != generation {
                    return None;
                }
                match event {
                    TerminalPtyEvent::Output(bytes) => {
                        let logging_error =
                            pane.log.as_mut().and_then(|log| log.write(&bytes).err());
                        if logging_error.is_some() {
                            pane.log = None;
                        }
                        pane.screen_mut().process_bytes(&bytes);
                        // Bumped directly rather than through `mark_dirty`: `pane` holds a mutable
                        // borrow of `self.panes`, and a disjoint field assignment is what the
                        // borrow checker accepts here.
                        let semantic_events = pane.screen_without_change().drain_semantic_events();
                        // Clipboard policy belongs to each attached client. The server parser exists
                        // for replay/runtime metadata, so never retain or apply its duplicate events.
                        pane.screen_without_change().drain_clipboard_events();
                        let responses = pane.screen_without_change().drain_responses();
                        if let Some(pty) = &pane.pty {
                            for response in responses {
                                if pane.initial_cursor_report_primed
                                    && is_cursor_position_report(&response)
                                {
                                    pane.initial_cursor_report_primed = false;
                                    continue;
                                }
                                let _ = pty.write(&response);
                            }
                        }
                        if owner.is_none() {
                            self.dirty_generation = self.dirty_generation.saturating_add(1);
                        }
                        let output = ServerOutbound::PaneOutput {
                            pane_id: id,
                            local: wire_local(owner),
                            generation,
                            bytes: bytes.to_vec(),
                        };
                        self.send_outbound(owner, &output);
                        if let Some(error) = logging_error {
                            self.send_outbound(
                                owner,
                                &ServerOutbound::control(ServerMessage::PaneLoggingChanged {
                                    pane_id: id,
                                    local: wire_local(owner),
                                    generation,
                                    enabled: false,
                                    path: None,
                                    error: Some(error),
                                }),
                            );
                        }
                        if !semantic_events.is_empty() {
                            self.sync_pane_runtime(owner, id, generation);
                        }
                        None
                    }
                    TerminalPtyEvent::Exited(code) => {
                        pane.pty = None;
                        // `command.is_some()` is what divides this from the client's
                        // `[pane] hold_on_exit`: a pane launched with a command is held here, as a
                        // live shell with the command's output above it, while a plain shell pane
                        // has nothing to hold open and falls through to the client, which decides
                        // whether to retain the exited husk. The two never both apply.
                        let keep_open =
                            pane.keep_open && pane.command.is_some() && !pane.command_completed;
                        if keep_open {
                            let outbound = if id == crate::state::POPUP_PANE_ID {
                                self.retain_completed_popup(owner, id, generation, code)
                            } else {
                                self.replace_with_keep_open_shell(owner, id, generation, code)
                            };
                            if let (Some(owner), Some(outbound)) = (owner, outbound.as_ref()) {
                                self.enqueue_outbound(owner, outbound);
                                return None;
                            }
                            return outbound;
                        }
                        pane.exited = Some(code);
                        if owner.is_none() {
                            self.mark_dirty();
                        }
                        self.sync_pane_runtime(owner, id, generation);
                        let outbound = ServerOutbound::control(ServerMessage::Exited {
                            pane_id: id,
                            local: wire_local(owner),
                            generation,
                            code,
                        });
                        if let Some(owner) = owner {
                            self.enqueue_outbound(owner, &outbound);
                            None
                        } else {
                            Some(outbound)
                        }
                    }
                    TerminalPtyEvent::Error(message) => {
                        let outbound = ServerOutbound::control(ServerMessage::SpawnResult {
                            pane_id: id,
                            local: wire_local(owner),
                            generation,
                            pid: None,
                            ok: false,
                            error: Some(message.to_string()),
                        });
                        if let Some(owner) = owner {
                            self.enqueue_outbound(owner, &outbound);
                            None
                        } else {
                            Some(outbound)
                        }
                    }
                }
            }
        }
    }

    /// Preserve a popup's final screen after its command exits without turning the transient
    /// result into an interactive shell.
    fn retain_completed_popup(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
        generation: u64,
        code: i32,
    ) -> Option<ServerOutbound> {
        let outcome = if code == 0 {
            "done".to_string()
        } else {
            format!("exit {code}")
        };
        let banner = format!("\r\n\x1b[2m[{outcome}]  Enter/Esc/Space: close\x1b[0m\r\n");
        let bytes = banner.into_bytes();
        let pane = self.pane_mut(owner, id)?;
        pane.screen_mut().process_bytes(&bytes);
        if let Some(log) = pane.log.as_mut() {
            let _ = log.write(&bytes);
        }
        pane.exited = Some(code);

        if owner.is_none() {
            self.mark_dirty();
        }
        self.sync_pane_runtime(owner, id, generation);
        self.send_outbound(
            owner,
            &ServerOutbound::PaneOutput {
                pane_id: id,
                local: wire_local(owner),
                generation,
                bytes,
            },
        );
        Some(ServerOutbound::control(ServerMessage::Exited {
            pane_id: id,
            local: wire_local(owner),
            generation,
            code,
        }))
    }

    /// A `keep_open` pane's command has exited: report its status into the pane's own output, then
    /// replace the dead PTY with the interactive shell so the pane stays usable (cross-platform plan
    /// Phase 4, "server-driven PTY replacement").
    ///
    /// Doing this server-side, rather than by appending `; exec <shell>` to the command line, is
    /// what makes three things true at once:
    ///
    /// - The exit status is *observed* here, so it can be shown. A shell that `exec`s over itself
    ///   has already discarded it.
    /// - **Scrollback survives.** The pane id and generation are unchanged and the `TerminalScreen`
    ///   is never recreated, so every client simply keeps appending to the buffer it already has -
    ///   the command's output is still there above the new shell's first prompt. The replacement is
    ///   invisible to a client; it just sees more bytes on the same pane.
    /// - It is shell-agnostic, and so works on Windows, where neither `exec` nor `;` means anything.
    ///
    /// If the replacement shell cannot be spawned, the pane exits for real with the command's
    /// status - the same outcome as a pane that was never `keep_open`.
    fn replace_with_keep_open_shell(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
        generation: u64,
        code: i32,
    ) -> Option<ServerOutbound> {
        // Dim, bracketed, and prefixed so it cannot be mistaken for output of the command itself.
        let banner = format!("\r\n\x1b[2m[rozi] command exited with status {code}\x1b[0m\r\n");
        let bytes = banner.into_bytes();

        let pane = self.pane_mut(owner, id)?;
        pane.screen_mut().process_bytes(&bytes);
        if let Some(log) = pane.log.as_mut() {
            let _ = log.write(&bytes);
        }

        // `spawnable_cwd` prefers the pane's live tracked cwd (where the command actually left it)
        // over its launch directory, and never returns a remote one.
        let cwd = pane.spawnable_cwd();
        let (cols, rows) = (pane.cols, pane.rows);
        let mut config = pty_config(None, &pane.shell, &[]).size(cols.max(1), rows.max(1));
        if let Some(cwd) = cwd.filter(|cwd| Path::new(cwd).is_dir()) {
            config = config.cwd(cwd);
        }
        for (key, value) in &pane.env {
            config = config.env(key.clone(), value.clone());
        }
        let events = Arc::clone(&self.events);
        let spawned = TerminalPty::spawn(config, move |event| {
            let event = ServerEvent::Pty(owner, id, generation, event);
            let bytes = event.payload_bytes();
            let _ = events.push_blocking_with(event, bytes, ServerEvent::coalesce_output);
        });

        let pane = self.pane_mut(owner, id)?;
        match spawned {
            Ok(pty) => {
                pane.initial_cursor_report_primed = cfg!(windows);
                pane.pty = Some(pty);
                pane.command_completed = true;
                pane.exited = None;
                // The pane id and generation survive this swap, so nothing else clears what was
                // learned about the command that just exited. A held agent state, a cached
                // detection fingerprint, or a stale `detected_agent` would otherwise be attributed
                // to the shell now sitting in its place - which is a different program.
                pane.agent = AgentScratch::default();
                pane.runtime.detected_agent = None;
                pane.runtime.work_started_at = None;
            }
            Err(_) => pane.exited = Some(code),
        }
        let died = pane.exited;

        if owner.is_none() {
            self.mark_dirty();
        }
        self.sync_pane_runtime(owner, id, generation);
        if died.is_some() {
            self.send_outbound(
                owner,
                &ServerOutbound::PaneOutput {
                    pane_id: id,
                    local: wire_local(owner),
                    generation,
                    bytes,
                },
            );
            return Some(ServerOutbound::control(ServerMessage::Exited {
                pane_id: id,
                local: wire_local(owner),
                generation,
                code,
            }));
        }
        Some(ServerOutbound::PaneOutput {
            pane_id: id,
            local: wire_local(owner),
            generation,
            bytes,
        })
    }

    pub(super) fn live_pane_mut(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
        generation: u64,
    ) -> Option<&mut ServerPane> {
        self.pane_mut(owner, id)
            .filter(|pane| pane.generation == generation && pane.exited.is_none())
    }

    pub(super) fn pane_mut(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
    ) -> Option<&mut ServerPane> {
        match owner {
            Some(owner) => self.local_panes.get_mut(&(owner, id)),
            None => self.panes.get_mut(&id),
        }
    }

    pub(super) fn pane(&self, owner: Option<ClientId>, id: PaneId) -> Option<&ServerPane> {
        match owner {
            Some(owner) => self.local_panes.get(&(owner, id)),
            None => self.panes.get(&id),
        }
    }

    fn remove_owned_pane(&mut self, owner: Option<ClientId>, id: PaneId) -> Option<ServerPane> {
        match owner {
            Some(owner) => self.local_panes.remove(&(owner, id)),
            None => self.panes.remove(&id),
        }
    }

    fn insert_owned_pane(&mut self, owner: Option<ClientId>, id: PaneId, pane: ServerPane) {
        match owner {
            Some(owner) => {
                self.local_panes.insert((owner, id), pane);
            }
            None => {
                self.panes.insert(id, pane);
            }
        }
    }

    pub(super) fn send_outbound(&mut self, owner: Option<ClientId>, outbound: &ServerOutbound) {
        if let Some(owner) = owner {
            self.enqueue_outbound(owner, outbound);
        } else {
            self.broadcast_outbound(outbound);
        }
    }

    pub(super) fn pane_meta(&self) -> Vec<PaneMeta> {
        self.panes
            .iter()
            .map(|(pane_id, pane)| PaneMeta {
                pane_id: *pane_id,
                generation: pane.generation,
                cols: pane.cols,
                rows: pane.rows,
                pid: pane.pty.as_ref().and_then(TerminalPty::pid),
                title: pane.effective_title(),
                original_user: Some(crate::platform::user::current_user_label()),
                exited: pane.exited,
                logging: pane.log.is_some(),
                runtime: pane.runtime.clone(),
            })
            .collect()
    }

    pub(super) fn set_pane_logging(
        &mut self,
        owner: Option<ClientId>,
        id: PaneId,
        generation: u64,
        enabled: bool,
    ) -> ServerMessage {
        let local = wire_local(owner);
        let session_name = self.session_name.clone();
        let settings = self.settings.clone();
        let Some(pane) = self
            .pane_mut(owner, id)
            .filter(|pane| pane.generation == generation)
        else {
            return ServerMessage::PaneLoggingChanged {
                pane_id: id,
                local,
                generation,
                enabled: false,
                path: None,
                error: Some("pane not found".to_string()),
            };
        };
        if !enabled {
            pane.log = None;
            return ServerMessage::PaneLoggingChanged {
                pane_id: id,
                local,
                generation,
                enabled: false,
                path: None,
                error: None,
            };
        }
        let (cols, rows) = (pane.cols, pane.rows);
        let header = LogHeader {
            session: &session_name,
            pane_id: id,
            generation,
            cols,
            rows,
        };
        let limit = settings.log_max_bytes;
        let result = session_log_dir(&settings, &session_name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "state directory unavailable"))
            .and_then(|(root, dir)| {
                crate::platform::fs_security::ensure_private_dir(&root)?;
                crate::platform::fs_security::ensure_private_dir(&dir)?;
                let path = dir.join(format!("{id}-{generation}.log"));
                // Created and locked down before anything is written, so the header never lands in
                // a file that is briefly readable by anyone else.
                OpenOptions::new().create(true).append(true).open(&path)?;
                #[cfg(unix)]
                fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
                PaneLog::open(&path, limit, &header)
            });
        match result {
            Ok(log) => {
                let path = log.path().to_string_lossy().into_owned();
                pane.log = Some(log);
                ServerMessage::PaneLoggingChanged {
                    pane_id: id,
                    local,
                    generation,
                    enabled: true,
                    path: Some(path),
                    error: None,
                }
            }
            Err(error) => ServerMessage::PaneLoggingChanged {
                pane_id: id,
                local,
                generation,
                enabled: false,
                path: None,
                error: Some(error.to_string()),
            },
        }
    }

    /// Delete an ephemeral session's log directory as the server exits.
    ///
    /// An `eph-*` session is disposable by definition, so the logs it wrote are too - and unlike a
    /// named session there is no later attach that could want them. Without this they accumulate
    /// one orphaned directory per rozi run, forever, holding whatever those panes printed.
    /// Named sessions are left alone: their logs are the point.
    pub(super) fn discard_ephemeral_logs(&mut self) {
        if !crate::state::is_ephemeral_session_name(&self.session_name) {
            return;
        }
        // Close every handle first: Windows refuses to delete a file that is still open.
        for pane in self.panes.values_mut() {
            pane.log = None;
        }
        let Some((_, dir)) = session_log_dir(&self.settings, &self.session_name) else {
            return;
        };
        if let Err(error) = fs::remove_dir_all(&dir)
            && error.kind() != io::ErrorKind::NotFound
        {
            eprintln!("rozi: could not remove session log directory: {error}");
        }
    }
}

pub(super) fn is_cursor_position_report(bytes: &[u8]) -> bool {
    let Some(body) = bytes
        .strip_prefix(b"\x1b[")
        .and_then(|bytes| bytes.strip_suffix(b"R"))
    else {
        return false;
    };
    let Some(separator) = body.iter().position(|byte| *byte == b';') else {
        return false;
    };
    let (row, col) = body.split_at(separator);
    !row.is_empty()
        && col.len() > 1
        && row.iter().all(u8::is_ascii_digit)
        && col[1..].iter().all(u8::is_ascii_digit)
}

/// The log root and one session's directory beneath it, or `None` when no state directory can be
/// resolved at all. Takes the two fields it needs rather than `&self` so it stays callable while a
/// pane is mutably borrowed out of `self.panes`.
fn session_log_dir(settings: &ServerSettings, session_name: &str) -> Option<(PathBuf, PathBuf)> {
    let root = settings.log_dir.clone().or_else(default_log_dir)?;
    let dir = root.join(session_name);
    Some((root, dir))
}

fn default_log_dir() -> Option<PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    if env.home.is_none() && env.xdg_state_home.is_none() {
        return None;
    }
    Some(crate::platform::paths::state_dir(&env).join("logs"))
}

/// The directory a pane's child actually starts in, set explicitly so it is *known* (and can be
/// reported) rather than inherited: the requested cwd when it exists, else the user's home, else the
/// server's own working directory.
///
/// A pane the client supplies no directory for — a `--remote` pane, whose local launch cwd is
/// meaningless on the server — otherwise has no cwd to display, since a remote pane has no shell
/// integration or process inspection to discover a live one (notably on Windows). Home is preferred
/// over `current_dir` because a detached Windows server can land on a working directory that
/// `current_dir` cannot even read.
fn effective_spawn_cwd(request_cwd: Option<&str>) -> Option<String> {
    request_cwd
        .filter(|cwd| Path::new(cwd).is_dir())
        .map(str::to_string)
        .or_else(crate::platform::paths::home_directory)
        .or_else(|| {
            std::env::current_dir()
                .ok()
                .map(|path| path.to_string_lossy().into_owned())
        })
}

#[cfg(test)]
mod tests {
    use super::effective_spawn_cwd;

    /// A directory-less spawn (a `--remote` pane) must still resolve *some* real directory so the
    /// pane reports where it started instead of showing no location — the bug this fixes on Windows.
    #[test]
    fn a_directoryless_spawn_still_resolves_a_cwd() {
        let resolved = effective_spawn_cwd(None).expect("home or current_dir resolves");
        assert!(
            std::path::Path::new(&resolved).is_dir(),
            "the fallback cwd must be a real directory, got {resolved:?}"
        );
    }

    #[test]
    fn a_valid_requested_cwd_is_used_verbatim() {
        let tmp = std::env::temp_dir();
        let tmp = tmp.to_string_lossy();
        assert_eq!(effective_spawn_cwd(Some(&tmp)), Some(tmp.into_owned()));
    }

    #[test]
    fn a_nonexistent_requested_cwd_falls_back_to_a_real_directory() {
        // A local launch path a different-OS server cannot use (e.g. a Linux path on Windows) must
        // not be passed through; it falls back to a directory that exists on the server.
        let resolved =
            effective_spawn_cwd(Some("/no/such/path/rozi-should-not-exist")).expect("fallback");
        assert!(std::path::Path::new(&resolved).is_dir());
        assert_ne!(resolved, "/no/such/path/rozi-should-not-exist");
    }
}
