//! Wire format between a client and the extension runtime it runs on a host.
//!
//! One dedicated SSH channel per client and host carries everything: control messages as JSON,
//! bundle bytes, and the byte streams of every bridged control connection, multiplexed by id. A
//! frame is a big-endian `u32` length, a kind byte, and the payload. The runtime trusts nothing it
//! is sent beyond what it is told to run; the client trusts nothing the runtime says beyond relaying
//! bytes, which it authenticates itself.

use std::io::{self, Read, Write};

use serde::{Deserialize, Serialize};

/// Protocol versions this build speaks.
pub const VERSION: u32 = 1;

/// The capability a host's Rozi advertises when it can run placed extension processes. Also
/// listed by `rozi api describe`.
pub const CAPABILITY: &str = "remote-extension-runtime";

/// Largest frame either side accepts. Bundles travel in [`STAGE_CHUNK`]-sized pieces and bridged
/// control traffic is line-oriented JSON, so nothing legitimate comes close.
pub const MAX_FRAME: usize = 4 * 1024 * 1024;

/// Bundle bytes per stage frame.
pub const STAGE_CHUNK: usize = 1024 * 1024;

const KIND_MESSAGE: u8 = 0;
const KIND_BRIDGE: u8 = 1;
const KIND_STAGE: u8 = 2;

/// Opening message, both ways.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_min: u32,
    pub protocol_max: u32,
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// The host's operating system, as Rust names it. Only the runtime fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    /// The runtime's Rozi version. Only the runtime fills it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

impl Hello {
    pub fn current(os: Option<String>) -> Self {
        Self {
            protocol_min: VERSION,
            protocol_max: VERSION,
            capabilities: vec![CAPABILITY.to_string()],
            version: os.as_ref().map(|_| env!("CARGO_PKG_VERSION").to_string()),
            os,
        }
    }

    pub fn validate(&self) -> io::Result<()> {
        crate::session::protocol::negotiate_protocol(
            VERSION,
            VERSION,
            self.protocol_max,
            self.protocol_min,
        )
        .map_err(|err| io::Error::new(io::ErrorKind::Unsupported, err.message()))?;
        if !self.capabilities.iter().any(|item| item == CAPABILITY) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                format!("peer does not offer {CAPABILITY}"),
            ));
        }
        Ok(())
    }
}

/// What a placed process runs, already resolved against nothing: `{extension_dir}` and `./`
/// paths are resolved by the runtime against the bundle it stages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Launch {
    Direct { argv: Vec<String> },
    Shell { line: String },
}

impl From<&crate::config::LaunchTemplate> for Launch {
    fn from(template: &crate::config::LaunchTemplate) -> Self {
        match template {
            crate::config::LaunchTemplate::Direct(argv) => Self::Direct { argv: argv.clone() },
            crate::config::LaunchTemplate::Shell(line) => Self::Shell { line: line.clone() },
        }
    }
}

impl Launch {
    pub fn template(&self) -> crate::config::LaunchTemplate {
        match self {
            Self::Direct { argv } => crate::config::LaunchTemplate::Direct(argv.clone()),
            Self::Shell { line } => crate::config::LaunchTemplate::Shell(line.clone()),
        }
    }
}

/// Where a placed process starts on the host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SpawnCwd {
    /// A directory inside the extension's files.
    Bundle { path: String },
    /// A directory on the host, such as the focused pane's. If it no longer exists the process
    /// starts in the runtime's own directory instead: the cwd belongs to the machine running the
    /// process, and never to another one.
    Host { path: String },
    /// No particular directory: the runtime's own, which is the host user's home over SSH.
    Inherit,
}

/// Collect a one-shot process's standard output, as a sidebar tab listing does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capture {
    pub max_bytes: usize,
    pub timeout_ms: u64,
}

/// Why a placed process could not be started. Each is shown to the user as the reason a
/// contribution is unavailable; none of them is ever answered by running it somewhere else.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
pub enum SpawnFailure {
    UnsupportedPlatform { os: String },
    MissingExecutable { program: String },
    BundleMissing,
    BundleCorrupt { detail: String },
    SpawnFailed { detail: String },
}

impl std::fmt::Display for SpawnFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform { os } => {
                write!(f, "the extension does not support {os}")
            }
            Self::MissingExecutable { program } => {
                write!(f, "`{program}` is not installed or not on the PATH")
            }
            Self::BundleMissing => f.write_str("the extension's files are not staged"),
            Self::BundleCorrupt { detail } => {
                write!(
                    f,
                    "the staged extension files failed verification: {detail}"
                )
            }
            Self::SpawnFailed { detail } => write!(f, "could not start: {detail}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Message {
    /// Client: the bundle sent in stage frames for this digest is complete.
    StageCommit { digest: String },
    /// Runtime: the bundle is verified and cached.
    Staged { digest: String },
    /// Runtime: the bundle was refused.
    StageFailed { digest: String, detail: String },
    /// Client: start a placed process.
    Spawn {
        worker: u64,
        digest: String,
        launch: Launch,
        cwd: SpawnCwd,
        /// Environment for the process. The runtime adds `ROZI`, `ROZI_BIN`, `ROZI_SOCKET`,
        /// `ROZI_EXTENSION_DIR`, and the credential; anything here naming those is overridden.
        env: Vec<(String, String)>,
        credential: String,
        /// `[extension] platforms`; empty means every platform.
        platforms: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<Capture>,
    },
    /// Runtime: the process started.
    Spawned { worker: u64, pid: u32 },
    /// Runtime: the process could not be started.
    SpawnFailed {
        worker: u64,
        #[serde(flatten)]
        failure: SpawnFailure,
    },
    /// Runtime: the process ended, on its own or because it was killed.
    Exited {
        worker: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<i32>,
        #[serde(default)]
        killed: bool,
        #[serde(default)]
        timed_out: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<String>,
    },
    /// Client: stop a process and everything it started.
    Kill { worker: u64 },
    /// Runtime: a process connected to the extension bridge.
    BridgeOpen { conn: u64 },
    /// Either side: one end of a bridged connection closed.
    BridgeClose { conn: u64 },
    /// Runtime: it cannot go on and is about to end. Unlike a failure of one launch, every
    /// process placed through it is affected, and the client should connect again for a new
    /// runtime rather than give up on any of them.
    RuntimeFatal { detail: String },
}

/// One frame off the wire.
#[derive(Debug, PartialEq, Eq)]
pub enum Frame {
    Hello(Hello),
    Message(Message),
    Bridge { conn: u64, data: Vec<u8> },
    Stage { digest: String, data: Vec<u8> },
}

fn write_raw(writer: &mut impl Write, kind: u8, parts: &[&[u8]]) -> io::Result<()> {
    let length: usize = 1 + parts.iter().map(|part| part.len()).sum::<usize>();
    if length > MAX_FRAME {
        return Err(io::Error::other(
            "extension runtime frame exceeds size limit",
        ));
    }
    let mut buffer = Vec::with_capacity(4 + length);
    buffer.extend_from_slice(&(length as u32).to_be_bytes());
    buffer.push(kind);
    for part in parts {
        buffer.extend_from_slice(part);
    }
    writer.write_all(&buffer)?;
    writer.flush()
}

pub fn write_hello(writer: &mut impl Write, hello: &Hello) -> io::Result<()> {
    let json = serde_json::to_vec(hello).map_err(io::Error::other)?;
    write_raw(writer, KIND_MESSAGE, &[&json])
}

pub fn write_message(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    let json = serde_json::to_vec(message).map_err(io::Error::other)?;
    write_raw(writer, KIND_MESSAGE, &[&json])
}

pub fn write_bridge(writer: &mut impl Write, conn: u64, data: &[u8]) -> io::Result<()> {
    write_raw(writer, KIND_BRIDGE, &[&conn.to_be_bytes(), data])
}

pub fn write_stage(writer: &mut impl Write, digest: &str, data: &[u8]) -> io::Result<()> {
    write_raw(writer, KIND_STAGE, &[digest.as_bytes(), data])
}

/// Read the next frame. `hello` says whether the opening message is expected, which is the only
/// JSON frame that is not a [`Message`]. `Ok(None)` is a clean end of stream.
pub fn read_frame(reader: &mut impl Read, hello: bool) -> io::Result<Option<Frame>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "extension runtime frame has an invalid length",
        ));
    }
    let mut payload = vec![0u8; length];
    reader.read_exact(&mut payload)?;
    let kind = payload[0];
    let body = &payload[1..];
    let invalid = |detail: &str| io::Error::new(io::ErrorKind::InvalidData, detail.to_string());
    Ok(Some(match kind {
        KIND_MESSAGE if hello => {
            Frame::Hello(serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))?)
        }
        KIND_MESSAGE => Frame::Message(
            serde_json::from_slice(body).map_err(|error| invalid(&error.to_string()))?,
        ),
        KIND_BRIDGE => {
            let (conn, data) = body
                .split_first_chunk::<8>()
                .ok_or_else(|| invalid("bridge frame is too short"))?;
            Frame::Bridge {
                conn: u64::from_be_bytes(*conn),
                data: data.to_vec(),
            }
        }
        KIND_STAGE => {
            if body.len() < 64 {
                return Err(invalid("stage frame is too short"));
            }
            let (digest, data) = body.split_at(64);
            let digest = std::str::from_utf8(digest)
                .ok()
                .filter(|digest| super::bundle::is_digest(digest))
                .ok_or_else(|| invalid("stage frame names no digest"))?;
            Frame::Stage {
                digest: digest.to_string(),
                data: data.to_vec(),
            }
        }
        _ => return Err(invalid("unknown extension runtime frame")),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip() {
        let mut wire = Vec::new();
        write_hello(&mut wire, &Hello::current(Some("linux".to_string()))).unwrap();
        write_message(
            &mut wire,
            &Message::SpawnFailed {
                worker: 3,
                failure: SpawnFailure::MissingExecutable {
                    program: "python3".to_string(),
                },
            },
        )
        .unwrap();
        write_bridge(&mut wire, 9, b"{\"ok\":true}\n").unwrap();
        let digest = "a".repeat(64);
        write_stage(&mut wire, &digest, b"bytes").unwrap();

        let mut reader = wire.as_slice();
        let Some(Frame::Hello(hello)) = read_frame(&mut reader, true).unwrap() else {
            panic!("hello first");
        };
        hello.validate().unwrap();
        assert_eq!(hello.os.as_deref(), Some("linux"));
        assert_eq!(
            read_frame(&mut reader, false).unwrap(),
            Some(Frame::Message(Message::SpawnFailed {
                worker: 3,
                failure: SpawnFailure::MissingExecutable {
                    program: "python3".to_string()
                }
            }))
        );
        assert_eq!(
            read_frame(&mut reader, false).unwrap(),
            Some(Frame::Bridge {
                conn: 9,
                data: b"{\"ok\":true}\n".to_vec()
            })
        );
        assert_eq!(
            read_frame(&mut reader, false).unwrap(),
            Some(Frame::Stage {
                digest,
                data: b"bytes".to_vec()
            })
        );
        assert_eq!(read_frame(&mut reader, false).unwrap(), None);
    }

    #[test]
    fn oversized_or_malformed_frames_are_refused() {
        let oversized = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(read_frame(&mut oversized.as_slice(), false).is_err());
        let mut unknown = Vec::new();
        write_raw(&mut unknown, 9, &[b"x"]).unwrap();
        assert!(read_frame(&mut unknown.as_slice(), false).is_err());
        let mut short = Vec::new();
        write_raw(&mut short, KIND_STAGE, &[b"../../etc/passwd"]).unwrap();
        assert!(read_frame(&mut short.as_slice(), false).is_err());
    }

    #[test]
    fn a_peer_without_the_capability_or_with_another_protocol_is_refused() {
        let mut hello = Hello::current(None);
        hello.capabilities.clear();
        assert_eq!(
            hello.validate().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        let mut hello = Hello::current(None);
        hello.protocol_min = VERSION + 1;
        hello.protocol_max = VERSION + 1;
        assert!(hello.validate().is_err());
    }
}
