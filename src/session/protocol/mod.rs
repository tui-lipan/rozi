//! Session wire protocol.
//!
//! # Version negotiation
//!
//! [`PROTOCOL_VERSION`] is this build's maximum supported wire version.
//! [`MIN_SUPPORTED_PROTOCOL`] is the oldest version this build can still speak.
//!
//! Clients advertise both on [`ClientMessage::Attach`] / [`ClientMessage::Query`]. The server
//! chooses `effective = min(client_max, server_max)` and accepts only when that value sits in both
//! sides' supported ranges. The negotiated value is echoed as `effective_protocol` on
//! [`ServerMessage::Attached`] / [`ServerMessage::SessionInfo`].
//!
//! The two bounds are equal, so a peer either speaks this exact version or is turned away. The
//! negotiation machinery stays regardless: the day a range is worth supporting is the day it has to
//! already be on the wire.

pub mod constants;
pub mod framing;
pub mod messages;
pub mod negotiation;
pub mod pane_runtime;

pub use constants::*;
pub use framing::*;
pub use messages::*;
pub use negotiation::*;
pub use pane_runtime::*;

#[cfg(test)]
mod tests;
