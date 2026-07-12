//! Cross-platform server existence and process control (cross-platform plan Phase 5b).
//!
//! **Still a placeholder module** - nothing calls into it yet - but most of the Unix half of what
//! Phase 5b asks for already exists, just living inline at its call sites rather than gathered
//! behind this module. Status against the plan's five bullets:
//!
//! - Background server spawn on `--attach`/`--session` bootstrap: **done on Unix**
//!   (`session::bootstrap::attach_session_client` spawns `hyprmux --session <name> --server` with
//!   `Stdio::null()` on all three streams). Windows needs `DETACHED_PROCESS`/`CREATE_NO_WINDOW`
//!   with no inherited console - **not implemented** (Milestone 2).
//! - An authenticated protocol-level `Shutdown` control message as the primary stop mechanism:
//!   **done, already the primary mechanism**, predating this plan.
//!   `ClientMessage::Shutdown` is authenticated (controller-only, rejects read-only clients - see
//!   `session::server::connection::handle_message`) and is what `SessionClient::shutdown` and
//!   `ops::session::shutdown_session`'s graceful path send. `ops::session::shutdown_session` only
//!   falls back to a Unix `SIGTERM` (via `terminate_unresponsive_server`, Phase 5 IPC's
//!   `IpcConnection::peer_pid`) when the graceful protocol handshake itself fails (e.g. an
//!   incompatible older server that cannot even attach) - i.e. exactly the "SIGTERM as a Unix
//!   courtesy handler mapping to the same path" relationship the plan describes, already in place.
//! - Orphan containment via a Windows Job Object with kill-on-close: **not implemented**
//!   (Milestone 2; N/A on Unix, where SIGTERM asks the orphaned server to reap its own PTYs).
//! - Console control events (`SetConsoleCtrlHandler` on Windows; SIGHUP-to-detach on Unix):
//!   **not implemented on either platform**. No signal handler currently converts SIGHUP (e.g. the
//!   terminal emulator hosting the client closing) into a clean detach; an unhandled SIGHUP today
//!   terminates the client process, leaving a named session's server running (detach-equivalent by
//!   accident) but skipping the clean detach path (`profiles::persist_session_on_detach`, etc.).
//!   Genuinely deferred - it needs a signal-to-message bridge onto the app's `CommandLink`, not
//!   attempted here to avoid rushing async-signal-context-unsafe code.
//! - Stale-server recovery (liveness probe, registry cleanup, forced termination):
//!   **substantially done on Unix**. `session::discovery::query_session_endpoint` probes via the
//!   protocol handshake and unlinks a socket file with no listener behind it; `EndpointRegistry`
//!   endpoint construction plus `IpcEndpoint::bind`'s stale-socket replacement cover the
//!   registry-cleanup half; forced termination is the same `SIGTERM` path above. `SIGKILL`
//!   escalation if `SIGTERM` is ignored is **not implemented** (no current call site needs it -
//!   the fallback is already a last resort after the graceful path failed).
//!
//! Net: for Milestone 1 (Unix-only), the only genuinely open item is SIGHUP-to-detach. Everything
//! else is real, tested, working Unix behavior that a future pass can still choose to consolidate
//! behind this module for symmetry with the Windows backend once that lands in Milestone 2.
