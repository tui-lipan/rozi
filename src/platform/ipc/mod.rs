//! Transport-neutral local IPC abstraction (cross-platform plan Phase 5).
//!
//! **Not implemented yet.** This is a placeholder for the transport-neutral types the plan
//! specifies (`IpcEndpoint`, `IpcListener`, `IpcConnection`, `BoundEndpoint`,
//! `EndpointRegistry`), which are meant to replace direct Unix socket types currently used
//! unconditionally in `main.rs`, `control.rs`, `cli.rs`, `session/client.rs`,
//! `session/discovery.rs`, `session/server/*`, and `ops/session.rs`. None of those call sites
//! have been migrated - they still use `std::os::unix::net::{UnixListener, UnixStream}` directly.
//! `unix` and `windows` are declared as empty per-backend submodules matching the plan's file
//! layout, ready to receive real implementations without further module-structure churn.

#[cfg(unix)]
pub mod unix;
#[cfg(windows)]
pub mod windows;
