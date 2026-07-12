//! macOS `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! **Not implemented yet and untested** - this environment has no macOS runtime to verify
//! against. Placeholder module matching the plan's file layout; the intended implementation uses
//! `proc_pidvnodepathinfo` for cwd, and `tcgetpgrp` plus `proc_name`/`proc_pidpath` for the
//! foreground executable. Only compiled under `cfg(target_os = "macos")` (see `process/mod.rs`),
//! so it cannot affect the Linux build either way.
