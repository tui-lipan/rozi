//! Cross-version session tests against a real released binary.
//!
//! Ignored, and a no-op even with `--ignored`, unless `ROZI_COMPAT=1`. Ordinary `cargo test` and
//! CI never download a release. `ROZI_COMPAT_OLD_BIN` points at a local binary instead of the
//! signed archive; `ROZI_COMPAT_OLD_VERSION` selects which release to fetch (default `0.0.43`).

#[path = "suites/compat/flows.rs"]
mod flows;
#[path = "suites/compat/harness.rs"]
mod harness;
#[path = "suites/compat/mismatch.rs"]
mod mismatch;
#[path = "suites/compat/remote.rs"]
mod remote;
#[path = "suites/compat/upgrade.rs"]
mod upgrade;
