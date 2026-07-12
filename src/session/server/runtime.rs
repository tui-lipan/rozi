//! Pane runtime-state computation (cross-platform plan Phase 6/7).
//!
//! [`SessionServer::sync_pane_runtime`] is the single call site the rest of `session::server`
//! should use: it re-derives a pane's [`protocol::PaneRuntimeState`] from its current
//! `TerminalScreen` semantic state plus the [`ProcessInspector`] fallback, and - only when
//! something actually changed - stores the new value and broadcasts
//! [`ServerMessage::PaneRuntimeChanged`].

use super::*;
use crate::platform::process::{PlatformProcessInspector, ProcessInspector};
use crate::session::protocol::{PaneCommandPhase, PaneCwdSource, PaneRuntimeState};

impl SessionServer {
    /// Recompute `pane_id`'s runtime state and broadcast a
    /// [`ServerMessage::PaneRuntimeChanged`] if it changed. `generation` must match the pane's
    /// current generation - a stale caller (e.g. a queued event racing a respawn) is a silent
    /// no-op, matching every other per-pane event handler in this module.
    pub(super) fn sync_pane_runtime(&mut self, pane_id: PaneId, generation: u64) {
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };
        if pane.generation != generation {
            return;
        }
        let inspector = PlatformProcessInspector::default();
        let next = compute_runtime_state(pane, &inspector);
        if next == pane.runtime {
            return;
        }
        pane.runtime = next.clone();
        self.broadcast_control(&ServerMessage::PaneRuntimeChanged {
            pane_id,
            generation,
            state: next,
        });
    }
}

/// Build the candidate [`PaneRuntimeState`] for `pane`, bumping `sequence` past its previous value
/// only when some other field actually differs (a no-op recompute must not burn a sequence number,
/// or every idle heartbeat tick would look like a change to clients).
fn compute_runtime_state(pane: &ServerPane, inspector: &impl ProcessInspector) -> PaneRuntimeState {
    let semantic = pane.screen.semantic_state();
    let command_phase = PaneCommandPhase::from(semantic.command_phase);
    let last_exit_status = match command_phase {
        PaneCommandPhase::Completed { exit_status } => {
            exit_status.or(pane.runtime.last_exit_status)
        }
        _ => pane.runtime.last_exit_status,
    };
    // A shell without OSC 133 integration never reports a per-command exit status; once the whole
    // pane exits, its own exit code is at least as informative a fallback as leaving this `None`.
    let last_exit_status = last_exit_status.or(pane.exited);
    let (cwd, cwd_host, cwd_source) = resolve_cwd(pane, semantic.cwd.as_ref(), inspector);
    let foreground_program = semantic
        .executable
        .as_deref()
        .map(str::to_string)
        .or_else(|| {
            pane.pty
                .as_ref()
                .and_then(|pty| inspector.foreground_program(pty))
        });

    let candidate = PaneRuntimeState {
        cwd,
        cwd_host,
        cwd_source,
        command_phase,
        foreground_program,
        last_exit_status,
        sequence: pane.runtime.sequence,
    };
    let changed = !cwd_unchanged(&candidate.cwd, &pane.runtime.cwd)
        || candidate.cwd_host != pane.runtime.cwd_host
        || candidate.cwd_source != pane.runtime.cwd_source
        || candidate.command_phase != pane.runtime.command_phase
        || candidate.foreground_program != pane.runtime.foreground_program
        || candidate.last_exit_status != pane.runtime.last_exit_status;
    PaneRuntimeState {
        sequence: if changed {
            pane.runtime.sequence.wrapping_add(1)
        } else {
            pane.runtime.sequence
        },
        ..candidate
    }
}

/// Resolve the pane's displayable cwd per [`PaneCwdSource`]'s precedence order: a valid local or
/// remote shell report first, then the process-inspector fallback (always local by construction -
/// it reads the actual child process, never a remote one), then the pane's original launch
/// directory. An invalid report (non-UTF-8 path bytes given the JSON wire encoding, or a
/// non-absolute path) falls through to the next tier rather than being repaired, per the plan.
fn resolve_cwd(
    pane: &ServerPane,
    reported: Option<&TerminalWorkingDirectory>,
    inspector: &impl ProcessInspector,
) -> (Option<String>, Option<String>, PaneCwdSource) {
    if let Some(reported) = reported
        && let Some(path) = decode_reported_path(reported)
    {
        let host = (!is_local_host(reported.host.as_deref()))
            .then(|| reported.host.as_deref().unwrap_or_default().to_string());
        return (Some(path), host, PaneCwdSource::ShellReport);
    }
    if let Some(pty) = &pane.pty
        && let Some(path) = inspector.cwd(pty)
    {
        return (
            Some(path.to_string_lossy().to_string()),
            None,
            PaneCwdSource::ProcessInspector,
        );
    }
    (pane.cwd.clone(), None, PaneCwdSource::LaunchDirectory)
}

/// Whether two resolved cwds name the same directory, under the platform's own comparison rules
/// (case-insensitive on Windows). A pane whose shell reports `c:\users\x` where its launch
/// directory was recorded as `C:\Users\x` has not changed directory, and must not burn a sequence
/// number - and therefore a `PaneRuntimeChanged` broadcast to every client - saying it has.
fn cwd_unchanged(candidate: &Option<String>, current: &Option<String>) -> bool {
    match (candidate, current) {
        (Some(candidate), Some(current)) => crate::platform::paths::paths_equal(candidate, current),
        (None, None) => true,
        _ => false,
    }
}

/// Percent-decoded `OSC 7`/`OSC 9;9` path bytes are exposed as raw bytes by the framework (never
/// lossy-converted) so a *local* filesystem operation could handle non-UTF-8 bytes exactly; this
/// wire protocol's control channel is JSON text, though, so a report that is not valid UTF-8
/// cannot be represented here without exactly the silent lossy conversion the plan forbids -
/// treated as an invalid report and left to fall through to the next precedence tier instead.
///
/// A decodable report is then normalized (and validated) by
/// [`crate::platform::paths::normalize_reported_cwd`], which is what applies the Unix leading-`/`
/// rule and the Windows drive-letter/UNC rules.
fn decode_reported_path(reported: &TerminalWorkingDirectory) -> Option<String> {
    crate::platform::paths::normalize_reported_cwd(reported.path_str()?)
}

/// Whether an `OSC 7` `host` component names this machine (`None`/empty/`localhost` always count
/// as local). Falls back to treating an unresolvable local hostname as "not local" - the safer
/// direction, since it only means a genuinely local report loses its `ShellReport` tier rather
/// than a remote one being mistaken for local.
fn is_local_host(host: Option<&str>) -> bool {
    match host {
        None => true,
        Some(host) if host.is_empty() || host.eq_ignore_ascii_case("localhost") => true,
        Some(host) => {
            crate::platform::user::hostname().is_some_and(|local| local.eq_ignore_ascii_case(host))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_pane() -> ServerPane {
        ServerPane {
            generation: 1,
            title: None,
            cwd: Some("/launch/dir".to_string()),
            command: None,
            keep_open: false,
            command_completed: false,
            shell: Vec::new(),
            env: Vec::new(),
            palette: WirePalette::from(TerminalColorPalette::default()),
            pty: None,
            screen: TerminalScreen::new(24, 80, 100),
            cols: 80,
            rows: 24,
            exited: None,
            log: None,
            runtime: PaneRuntimeState::default(),
        }
    }

    struct StubInspector;
    impl ProcessInspector for StubInspector {
        fn cwd(&self, _pty: &TerminalPty) -> Option<std::path::PathBuf> {
            None
        }
        fn foreground_program(&self, _pty: &TerminalPty) -> Option<String> {
            None
        }
    }

    #[test]
    fn falls_back_to_launch_cwd_with_no_pty_and_no_shell_report() {
        let pane = make_pane();
        let state = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(state.cwd, Some("/launch/dir".to_string()));
        assert_eq!(state.cwd_source, PaneCwdSource::LaunchDirectory);
        assert_eq!(state.cwd_host, None);
        assert_eq!(state.sequence, 1);
    }

    #[test]
    fn a_local_osc7_report_wins_over_launch_cwd() {
        let mut pane = make_pane();
        pane.screen
            .process_bytes(b"\x1b]7;file://localhost/reported/dir\x1b\\");
        let state = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(state.cwd, Some("/reported/dir".to_string()));
        assert_eq!(state.cwd_source, PaneCwdSource::ShellReport);
        assert_eq!(state.cwd_host, None);
    }

    #[test]
    fn a_remote_osc7_report_is_displayable_but_flagged_with_a_host() {
        let mut pane = make_pane();
        pane.screen
            .process_bytes(b"\x1b]7;file://otherhost.example/remote/dir\x1b\\");
        let state = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(state.cwd, Some("/remote/dir".to_string()));
        assert_eq!(state.cwd_host, Some("otherhost.example".to_string()));
        assert_eq!(state.cwd_source, PaneCwdSource::ShellReport);
    }

    #[test]
    fn a_non_absolute_osc7_report_falls_through_to_launch_cwd() {
        let mut pane = make_pane();
        pane.screen
            .process_bytes(b"\x1b]7;file://localhost/relative%2fpath\x1b\\");
        // Force the report path itself to be non-absolute by using a bare relative form: OSC 7
        // URIs are always absolute per spec, so exercise the fallthrough via a directly
        // constructed report instead of a crafted escape sequence.
        let (cwd, host, source) = resolve_cwd(
            &pane,
            Some(&TerminalWorkingDirectory {
                host: None,
                path: std::sync::Arc::from(b"relative/path".as_slice()),
                source: TerminalWorkingDirectorySource::Osc7,
            }),
            &StubInspector,
        );
        assert_eq!(cwd, Some("/launch/dir".to_string()));
        assert_eq!(host, None);
        assert_eq!(source, PaneCwdSource::LaunchDirectory);
    }

    #[test]
    fn recompute_is_a_no_op_when_nothing_changed() {
        let mut pane = make_pane();
        let first = compute_runtime_state(&pane, &StubInspector);
        pane.runtime = first.clone();
        let second = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(second, first);
        assert_eq!(second.sequence, first.sequence);
    }

    #[test]
    fn completed_command_phase_sticks_the_exit_status() {
        let mut pane = make_pane();
        pane.screen.process_bytes(b"\x1b]133;D;7\x1b\\");
        let state = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(
            state.command_phase,
            PaneCommandPhase::Completed {
                exit_status: Some(7)
            }
        );
        assert_eq!(state.last_exit_status, Some(7));

        pane.runtime = state;
        pane.screen.process_bytes(b"\x1b]133;A\x1b\\");
        let next = compute_runtime_state(&pane, &StubInspector);
        assert_eq!(next.command_phase, PaneCommandPhase::Prompt);
        assert_eq!(next.last_exit_status, Some(7));
    }
}
