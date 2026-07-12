//! Windows `ProcessInspector` implementation (cross-platform plan Phase 9).
//!
//! **Not implemented and untested** - this environment has no Windows target installed. Per the
//! plan, Windows process inspection is intentionally unsupported (no PEB or process-tree probing);
//! this module is expected to end up as an explicit "always unavailable" implementation rather
//! than a real one. Only compiled under `cfg(windows)` (see `process/mod.rs`), so it cannot
//! affect the Linux build either way.
