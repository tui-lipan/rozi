//! Windows named-pipe IPC backend (cross-platform plan Phase 5).
//!
//! **Not implemented and untested** - this environment has no Windows target installed. Per the
//! plan, this backend is meant to use duplex byte-mode named pipes with flat, dot-separated names
//! (`\\.\pipe\hyprmux.<user-sid>.control.<pid>`, `\\.\pipe\hyprmux.<user-sid>.session.<name>`), an
//! explicit current-user SID DACL, `PIPE_REJECT_REMOTE_CLIENTS`, non-inheritable handles, and the
//! "fresh instance before each `ConnectNamedPipe`, `FILE_FLAG_FIRST_PIPE_INSTANCE` on the first
//! instance, fail closed otherwise" multi-instance accept pattern. Only compiled under
//! `cfg(windows)` (see `ipc/mod.rs`), so it cannot affect the Linux build either way.
