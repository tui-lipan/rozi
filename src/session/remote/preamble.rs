//! Framed preamble emitted by `--remote-serve` before the session protocol begins.

use serde::{Deserialize, Serialize};

use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};

pub const PREAMBLE_MAGIC: &str = "hyprmux-remote-serve";
pub const PREAMBLE_VERSION: u32 = 1;

/// Fixed header byte so preamble bytes are never mistaken for a session frame length prefix.
const PREAMBLE_KIND: u8 = b'R';

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePreamble {
    pub magic: String,
    pub version: u32,
    pub platform: String,
    pub hyprmux_version: String,
    pub protocol_max: u32,
    pub protocol_min: u32,
    /// True when this `--remote-serve` invocation started the session server (create_only identity).
    pub server_started: bool,
}

impl RemotePreamble {
    pub fn current(server_started: bool) -> Self {
        Self {
            magic: PREAMBLE_MAGIC.to_string(),
            version: PREAMBLE_VERSION,
            platform: std::env::consts::OS.to_string(),
            hyprmux_version: env!("CARGO_PKG_VERSION").to_string(),
            protocol_max: PROTOCOL_VERSION,
            protocol_min: MIN_SUPPORTED_PROTOCOL,
            server_started,
        }
    }

    pub fn validate_for_client(&self) -> Result<(), String> {
        if self.magic != PREAMBLE_MAGIC {
            return Err(format!(
                "remote proxy preamble magic mismatch (got {:?})",
                self.magic
            ));
        }
        if self.version != PREAMBLE_VERSION {
            return Err(format!(
                "remote proxy preamble version {} is unsupported (need {PREAMBLE_VERSION})",
                self.version
            ));
        }
        crate::session::protocol::negotiate_protocol(
            PROTOCOL_VERSION,
            MIN_SUPPORTED_PROTOCOL,
            self.protocol_max,
            self.protocol_min,
        )
        .map(|_| ())
        .map_err(|mismatch| {
            format!(
                "remote hyprmux protocol is incompatible ({})",
                mismatch.message()
            )
        })
    }
}

pub fn write_preamble<W: std::io::Write>(
    writer: &mut W,
    preamble: &RemotePreamble,
) -> std::io::Result<()> {
    let body = serde_json::to_vec(preamble).map_err(std::io::Error::other)?;
    let len: u32 = body
        .len()
        .try_into()
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "preamble too large"))?;
    writer.write_all(&[PREAMBLE_KIND])?;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()
}

pub fn read_preamble<R: std::io::Read>(reader: &mut R) -> std::io::Result<RemotePreamble> {
    let mut kind = [0u8; 1];
    reader.read_exact(&mut kind)?;
    if kind[0] != PREAMBLE_KIND {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "expected remote preamble kind {:?}, got {:?}",
                PREAMBLE_KIND as char, kind[0] as char
            ),
        ));
    }
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 64 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "remote preamble too large",
        ));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|err| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid remote preamble JSON: {err}"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preamble_round_trips() {
        let preamble = RemotePreamble::current(true);
        let mut buf = Vec::new();
        write_preamble(&mut buf, &preamble).unwrap();
        let decoded = read_preamble(&mut &buf[..]).unwrap();
        assert_eq!(decoded, preamble);
        decoded.validate_for_client().unwrap();
    }
}
