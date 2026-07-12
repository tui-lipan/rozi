//! Explicit platform abstraction layer (cross-platform plan Phase 2).
//!
//! Higher-level modules should reach OS-specific behavior (paths, private-directory security,
//! process inspection, local IPC, server lifecycle control, notifications) through here instead
//! of referencing `std::os::unix`, `/proc`, `SO_PEERCRED`, XDG/AppData env vars, Unix permission
//! bits, named-pipe APIs, or Unix signal APIs directly. That migration is incremental: see each
//! submodule's doc comment for what has actually landed versus what is still a placeholder.
//!
//! Status (tracked against the cross-platform plan):
//! - [`paths`] - **implemented**: config/state/cache/runtime directory resolution (Phase 3),
//!   wired into `config::file`, `profiles`, `session::server::resurrect`,
//!   `session::server::panes`, and `control::runtime_dir`.
//! - [`fs_security`] - **implemented for Unix**; Windows DACL/reparse-point handling is Phase 5/5b
//!   and currently only creates the directory without enforcing privacy.
//! - [`user`] - **implemented**: `current_user_tag()`, pulled forward from Phase 10 because
//!   [`paths`] needed it for the runtime-dir fallback path.
//! - [`command`] - **implemented** (Phase 4): interactive-shell/command-runner resolution, wired
//!   into every pane/hook/workbar/`[keys] run` spawn path.
//! - [`ipc`] - **implemented for Unix** (Phase 5): transport-neutral `IpcEndpoint`/`IpcListener`/
//!   `IpcConnection`/`BoundEndpoint`/`EndpointRegistry`, wired into `control.rs`,
//!   `session/client.rs`, `session/discovery.rs`, `session/server/*`, `cli.rs`, and
//!   `ops/session.rs`'s peer-pid probe. The Windows named-pipe backend is a type-matching stub only
//!   (Milestone 2).
//! - [`notifications`], [`server_lifecycle`], [`process`] - module skeletons only. No call site has
//!   been migrated to them yet; existing Unix-specific code (`ops/config.rs` executable checks, the
//!   Unix signal/process-control logic inline in `ops/session.rs`) still lives at its current call
//!   sites pending Phases 5b/9/10.

pub mod fs_security;
pub mod paths;
pub mod user;

pub mod command;
pub mod ipc;
pub mod notifications;
pub mod process;
pub mod server_lifecycle;
