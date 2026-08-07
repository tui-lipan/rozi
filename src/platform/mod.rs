//! Explicit platform abstraction layer (cross-platform plan Phase 2).
//!
//! Higher-level modules should reach OS-specific behavior (paths, private-directory security,
//! process inspection, local IPC, server lifecycle control, notifications) through here instead
//! of referencing `std::os::unix`, `/proc`, `SO_PEERCRED`, XDG/AppData env vars, Unix permission
//! bits, named-pipe APIs, or Unix signal APIs directly. That migration is incremental: see each
//! submodule's doc comment for what has actually landed versus what is still a placeholder.
//!
//! Status (tracked against the cross-platform plan):
//! - [`paths`] - config/state/cache/data/runtime directory resolution (Phase 3), wired into
//!   `config::file`, `profiles`, `session::server::resurrect`, `session::server::panes`,
//!   `platform::shell_integration`, and `control::runtime_dir`.
//! - [`fs_security`] - private-directory policy: Unix ownership/mode/symlink enforcement, Windows
//!   protected current-user-SID DACL plus reparse-point rejection (Phase 3/5). Re-exported from
//!   `relswap::fs::security` so session/control/IPC and the updater share one implementation. The
//!   Windows security descriptor is shared with the named-pipe backend, so an endpoint and the
//!   registry entry that advertises it carry the same ACL.
//! - [`user`] - current-user identity (Phase 10): `current_user_tag` (machine identity: uid on Unix,
//!   SID on Windows), `current_user_label` (`USER` vs `USERNAME`), and `hostname`.
//! - [`command`] - interactive-shell/command-runner resolution (Phase 4), wired into every
//!   pane/hook/workbar/`[keys] run` spawn path, plus Windows `PATH`/`PATHEXT` program lookup
//!   (Phase 10).
//! - [`ipc`] - transport-neutral `IpcEndpoint`/`IpcListener`/`IpcConnection`/`BoundEndpoint`/
//!   `EndpointRegistry` (Phase 5), wired into `control.rs`, `session/client.rs`,
//!   `session/discovery.rs`, `session/server/*`, `cli.rs`, and `ops/session.rs`. Unix-domain sockets
//!   on Linux/macOS; secured named pipes on Windows.
//! - [`server_lifecycle`] - background server spawn, hangup/console-control handling, protocol-first
//!   shutdown with a signal courtesy path, Windows Job Object orphan containment, and forced
//!   termination with `SIGKILL`/`TerminateProcess` escalation (Phase 5b).
//! - [`shell_integration`] - shipped per-shell integration scripts and their injection points
//!   (Phase 8): bash, zsh, fish, PowerShell, and cmd.
//! - [`process`] - the `ProcessInspector` trait plus per-OS `cwd`/`foreground_program` fallbacks
//!   (Phase 9), wired into `session::server`'s pane-runtime-state computation (Phase 6). Windows is
//!   an explicit "unavailable" implementation per the plan (no PEB or process-tree probing).
//! - [`notifications`] - desktop notifications (Phase 10).
//! - [`install`] - thin hyprmux path/policy adapter over the `relswap` signed-release engine.
//!
//! Every Windows code path in here type-checks under `cargo check --target x86_64-pc-windows-gnu`
//! and is exercised by Windows CI, but was **written without a Windows host in the authoring
//! workspace**; treat CI as the first real verification of any of it.

pub mod fs_security;
pub mod install;
pub mod paths;
pub mod user;

pub mod command;
pub mod ipc;
pub mod notifications;
pub mod process;
pub mod server_lifecycle;
pub mod shell_integration;
