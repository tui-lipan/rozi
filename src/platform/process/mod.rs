//! Foreground-process and working-directory inspection fallbacks (cross-platform plan Phase 9).
//!
//! **Not implemented yet.** This is a placeholder for the `ProcessInspector` trait the plan
//! specifies:
//!
//! ```text
//! trait ProcessInspector {
//!     fn cwd(&self, pty: &TerminalPty) -> Option<PathBuf>;
//!     fn foreground_program(&self, pty: &TerminalPty) -> Option<String>;
//! }
//! ```
//!
//! Today, Linux `/proc`-based foreground/cwd inspection (where it exists) lives inline at its
//! call sites rather than behind a trait; macOS (`proc_pidvnodepathinfo`, `tcgetpgrp`,
//! `proc_name`/`proc_pidpath`) and Windows (explicit "unavailable") implementations do not exist
//! yet. `linux`, `macos`, and `windows` are declared as empty per-OS submodules matching the
//! plan's file layout, ready to receive real implementations without further module-structure
//! churn.

#[cfg(target_os = "linux")]
pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(windows)]
pub mod windows;
