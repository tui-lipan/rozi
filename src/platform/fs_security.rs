//! Cross-platform "private directory" security policy.
//!
//! Implemented once in `relswap::fs::security` and re-exported here so session/control/IPC keep
//! calling through `platform::fs_security` without duplicating the symlink/reparse-point/DACL
//! rules.

pub use relswap::fs::security::*;
