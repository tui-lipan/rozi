//! Running an extension's processes away from the client.
//!
//! An extension is client-owned: this client loaded it, enabled it, and fences its generations.
//! Some of its processes belong beside a session rather than beside the UI - they read files,
//! run tools, or list agents on the machine the session runs on. This domain carries them there
//! and back: an immutable [`bundle`] of the loaded files, and the per-host runtime that runs them
//! and relays their control traffic to the client that owns them.
//!
//! It is not part of the session server. A session server outlives clients and must never run a
//! transient client's code; the runtime belongs to one client's connection to one host and dies
//! with it.

pub mod bundle;
pub mod client;
pub mod protocol;
pub mod server;
pub mod store;
