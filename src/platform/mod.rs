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
//! - [`command`], [`notifications`], [`server_lifecycle`], [`process`], [`ipc`] - module skeletons
//!   only. No call site has been migrated to them yet; existing Unix-specific code (`control.rs`
//!   socket binding, `ops/session.rs` peer-credential checks, `ops/config.rs` executable checks)
//!   still lives at its current call sites pending Phases 4/5/5b/9.

pub mod fs_security;
pub mod paths;
pub mod user;

pub mod command;
pub mod ipc;
pub mod notifications;
pub mod process;
pub mod server_lifecycle;
