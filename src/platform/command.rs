//! Shell and command-runner resolution (cross-platform plan Phase 4).
//!
//! **Not implemented yet.** This module is a placeholder marking where Phase 4's two resolved
//! launch policies belong:
//!
//! - Interactive shell resolution (`shell` config -> `$SHELL`/`/bin/sh` on Unix, or
//!   `pwsh.exe`/`powershell.exe`/`%COMSPEC%`/`cmd.exe` on Windows).
//! - A deterministic (never detection-based) `command_shell` for pane/popup/hook/workbar/`[keys]
//!   run`/profile/control-socket command execution.
//!
//! Today, session PTY spawning still has its own inline shell handling in `pane_lifecycle.rs`
//! and does not consistently honor the configured `shell` value - the exact bug Phase 4 is meant
//! to fix. Nothing currently calls into this module.
