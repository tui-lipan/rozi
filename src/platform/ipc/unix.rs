//! Unix-domain-socket IPC backend (cross-platform plan Phase 5).
//!
//! **Not implemented yet.** Placeholder module matching the plan's file layout. The intended
//! implementation preserves Unix sockets and the current private-directory protections
//! (`super::super::fs_security`) on Linux and macOS, with peer authentication via `SO_PEERCRED`
//! on Linux and `LOCAL_PEERCRED`/`getpeereid` on macOS. Existing direct
//! `std::os::unix::net::{UnixListener, UnixStream}` usage in `control.rs`, `session/client.rs`,
//! `session/discovery.rs`, `session/server/*`, `cli.rs`, `main.rs`, and `ops/session.rs` has not
//! been migrated behind this module yet.
