//! Cross-platform server existence and process control (cross-platform plan Phase 5b, new).
//!
//! **Not implemented yet.** Placeholder for what is currently covered only implicitly by Unix
//! signals/fork semantics in `ops/session.rs` (`libc::kill(pid, SIGTERM)` for orphan cleanup,
//! `SO_PEERCRED` for the graceful-attach handshake) and `session::server`'s direct process
//! spawning. Phase 5b calls for, at minimum:
//!
//! - Background server spawn on `--attach`/`--session` bootstrap (Unix detach semantics today;
//!   Windows needs `DETACHED_PROCESS`/`CREATE_NO_WINDOW` with no inherited console).
//! - An authenticated protocol-level `Shutdown` control message as the primary stop mechanism on
//!   every platform, with SIGTERM downgraded to a Unix courtesy handler mapping to the same path.
//! - Orphan containment on Windows via a Job Object with kill-on-close for the server and its
//!   ConPTY children.
//! - Console control events (`SetConsoleCtrlHandler` on Windows; SIGHUP-to-detach on Unix, which
//!   already exists but is not routed through this module).
//! - Stale-server recovery: a liveness probe, registry-entry cleanup, and forced termination
//!   (`SIGKILL` on Unix, `TerminateJobObject` on Windows).
//!
//! Nothing currently calls into this module; the existing Unix-only logic it should eventually
//! subsume still lives in `ops/session.rs` and `session/server/`.
