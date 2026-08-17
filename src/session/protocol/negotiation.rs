use crate::session::protocol::constants::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};
use crate::session::protocol::messages::ClientMessage;

/// Normalize a peer-advertised minimum: missing/`0` means "exactly `max`" (pre-negotiation peers).
pub fn normalize_min_protocol(max: u32, min: u32) -> u32 {
    if min == 0 { max } else { min }
}

/// Error from [`negotiate_protocol`] when the advertised ranges do not overlap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProtocolMismatch {
    pub client_max: u32,
    pub client_min: u32,
    pub server_max: u32,
    pub server_min: u32,
    pub older_side: ProtocolSide,
}

/// Which peer is too old for the other side's range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolSide {
    Client,
    Server,
}

impl ProtocolMismatch {
    pub fn message(&self) -> String {
        let older = match self.older_side {
            ProtocolSide::Client => "client",
            ProtocolSide::Server => "server",
        };
        format!(
            "client protocol {}-{} is incompatible with server protocol {}-{} ({older} is older)",
            self.client_min, self.client_max, self.server_min, self.server_max
        )
    }
}

/// Choose the effective wire version for a client/server pair.
///
/// `effective = min(client_max, server_max)`, accepted only when it falls within both sides'
/// supported ranges. A `client_min` of `0` is treated as "exactly `client_max`".
pub fn negotiate_protocol(
    client_max: u32,
    client_min: u32,
    server_max: u32,
    server_min: u32,
) -> std::result::Result<u32, ProtocolMismatch> {
    let client_min = normalize_min_protocol(client_max, client_min);
    let effective = client_max.min(server_max);
    if effective < client_min || effective < server_min {
        let older_side = if client_max < server_min {
            ProtocolSide::Client
        } else {
            ProtocolSide::Server
        };
        return Err(ProtocolMismatch {
            client_max,
            client_min,
            server_max,
            server_min,
            older_side,
        });
    }
    Ok(effective)
}

/// Attach message using this build's supported protocol range.
pub fn attach_message(
    session: impl Into<String>,
    label: impl Into<String>,
    read_only: bool,
    shares_filesystem: bool,
) -> ClientMessage {
    ClientMessage::Attach {
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
        label: label.into(),
        read_only,
        shares_filesystem,
    }
}

/// Query message using this build's supported protocol range.
pub fn query_message(session: impl Into<String>) -> ClientMessage {
    ClientMessage::Query {
        session: session.into(),
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: MIN_SUPPORTED_PROTOCOL,
    }
}
