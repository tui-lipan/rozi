//! Framed preamble emitted by `--remote-serve` before the session protocol begins.

use serde::{Deserialize, Serialize};

use crate::session::protocol::{MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION};

pub const PREAMBLE_MAGIC: &str = "hyprmux-remote-serve";
/// Newest preamble version this build emits and understands.
pub const PREAMBLE_VERSION: u32 = 1;
/// Oldest preamble version this build can still read. The preamble is the *first* compatibility
/// surface — parsed before protocol negotiation — so it negotiates over a range for the same
/// reason the wire protocol does: additive fields (each `#[serde(default)]`) let an older peer
/// ignore what it does not know, and only a breaking layout change bumps this floor.
pub const MIN_SUPPORTED_PREAMBLE: u32 = 1;

/// Fixed header byte so preamble bytes are never mistaken for a session frame length prefix.
const PREAMBLE_KIND: u8 = b'R';

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemotePreamble {
    pub magic: String,
    /// Newest preamble version the sender speaks (its maximum).
    pub version: u32,
    /// Oldest preamble version the sender still speaks. Defaults to `0` for a pre-range sender,
    /// which negotiation reads as "no lower bound", so an older `--remote-serve` still validates.
    #[serde(default)]
    pub preamble_min: u32,
    #[serde(default)]
    pub platform: String,
    #[serde(default)]
    pub hyprmux_version: String,
    #[serde(default)]
    pub protocol_max: u32,
    #[serde(default)]
    pub protocol_min: u32,
    /// True when this `--remote-serve` invocation started the session server (create_only identity).
    #[serde(default)]
    pub server_started: bool,
}

impl RemotePreamble {
    pub fn current(server_started: bool) -> Self {
        Self {
            magic: PREAMBLE_MAGIC.to_string(),
            version: PREAMBLE_VERSION,
            preamble_min: MIN_SUPPORTED_PREAMBLE,
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
        // Reuse the wire-protocol range comparator rather than a second bespoke check. The message
        // deliberately avoids the words "protocol"/"incompatible" so the caller does not misfile a
        // preamble-version skew as a session-protocol skew (which would trigger a pointless server
        // restart — the remote binary's version does not change on restart).
        crate::session::protocol::negotiate_protocol(
            PREAMBLE_VERSION,
            MIN_SUPPORTED_PREAMBLE,
            self.version,
            self.preamble_min,
        )
        .map_err(|_| {
            format!(
                "remote proxy preamble version {} (min {}) is unsupported by this client (speaks {MIN_SUPPORTED_PREAMBLE}-{PREAMBLE_VERSION})",
                self.version, self.preamble_min
            )
        })?;
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

/// How far to scan for the preamble's magic byte before giving up. Remote-side chatter — an SSH
/// login banner, an MOTD, or a shell rc that prints — arrives ahead of the framed preamble; this
/// bounds how much of it we will skip past.
const PREAMBLE_SCAN_WINDOW: usize = 8 * 1024;

/// Render skipped stream bytes as a short, readable suffix for an error message.
fn noise_suffix(discarded: &[u8]) -> String {
    if discarded.is_empty() {
        return String::new();
    }
    let shown = &discarded[..discarded.len().min(256)];
    let text = String::from_utf8_lossy(shown);
    let ellipsis = if discarded.len() > shown.len() {
        "…"
    } else {
        ""
    };
    format!(
        " (remote sent {} byte(s) of non-preamble output first: {:?}{ellipsis})",
        discarded.len(),
        text.trim()
    )
}

pub fn read_preamble<R: std::io::Read>(reader: &mut R) -> std::io::Result<RemotePreamble> {
    // Scan forward for the magic kind byte, discarding any preceding chatter within a bounded
    // window. A banner/MOTD before the framed preamble is one of the most common real-world SSH
    // transport failures; skipping it (and reporting what was skipped on failure) beats surfacing
    // an opaque "expected R, got <first banner char>".
    let mut discarded: Vec<u8> = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let read = reader.read(&mut byte)?;
        if read == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "remote stream closed before a preamble was seen{}",
                    noise_suffix(&discarded)
                ),
            ));
        }
        if byte[0] == PREAMBLE_KIND {
            break;
        }
        discarded.push(byte[0]);
        if discarded.len() >= PREAMBLE_SCAN_WINDOW {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "no remote preamble found within the first {PREAMBLE_SCAN_WINDOW} bytes{}",
                    noise_suffix(&discarded)
                ),
            ));
        }
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
            format!(
                "invalid remote preamble JSON: {err}{}",
                noise_suffix(&discarded)
            ),
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

    /// A login banner / MOTD / chatty rc file printed before the framed preamble is skipped, not
    /// treated as a corrupt read — this is the single most common real-world SSH transport failure.
    #[test]
    fn read_preamble_skips_leading_banner_noise() {
        let mut buf = Vec::new();
        buf.extend_from_slice(b"Welcome to the remote host!\r\nLast login: yesterday\n");
        write_preamble(&mut buf, &RemotePreamble::current(false)).unwrap();
        let decoded = read_preamble(&mut &buf[..]).expect("preamble found past the banner");
        assert_eq!(decoded.magic, PREAMBLE_MAGIC);
    }

    /// When no preamble ever arrives, the error names the discarded bytes so the cause is obvious
    /// instead of an opaque "expected R, got W".
    #[test]
    fn read_preamble_reports_discarded_bytes_on_failure() {
        let noise = b"error: command not found\n";
        let err = read_preamble(&mut &noise[..]).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert!(
            err.to_string().contains("command not found"),
            "error should surface the discarded output, got: {err}"
        );
    }

    /// A pre-range sender (only `version`, no `preamble_min`) still validates: the defaulted
    /// `preamble_min = 0` reads as "no lower bound".
    #[test]
    fn pre_range_preamble_without_min_still_validates() {
        let json = serde_json::json!({
            "magic": PREAMBLE_MAGIC,
            "version": PREAMBLE_VERSION,
            "platform": "linux",
            "hyprmux_version": "0.1.0",
            "protocol_max": crate::session::protocol::PROTOCOL_VERSION,
            "protocol_min": crate::session::protocol::MIN_SUPPORTED_PROTOCOL,
            "server_started": false,
        });
        let body = serde_json::to_vec(&json).unwrap();
        let mut framed = Vec::new();
        framed.push(PREAMBLE_KIND);
        framed.extend_from_slice(&(body.len() as u32).to_be_bytes());
        framed.extend_from_slice(&body);
        let decoded = read_preamble(&mut &framed[..]).unwrap();
        assert_eq!(decoded.preamble_min, 0);
        decoded.validate_for_client().unwrap();
    }
}
