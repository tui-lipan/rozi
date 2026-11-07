/// The wire generation this build speaks: its maximum supported session protocol.
///
/// A generation is a shape every peer of that generation can parse. It is bumped only for a change
/// the evolution rules in `dev/wire-compatibility.md` cannot express: removing or renaming
/// something, changing a type, a new required field, or a framing change. A bump is a breaking
/// change, marked `!` with a `### Breaking changes` section in the release notes, and development
/// builds follow the same rule. Everything else is additive: an optional field a receiver can do
/// without, or a [`super::Capability`] the sender checks before it relies on the new behavior.
///
/// The handshake ([`super::ClientMessage::Attach`], [`super::ClientMessage::Query`],
/// [`super::ClientMessage::SessionControl`], [`super::ServerMessage::Attached`],
/// [`super::ServerMessage::SessionInfo`], [`super::ServerMessage::Error`] with
/// [`PROTOCOL_MISMATCH_ERROR_CODE`], and the remote preamble) is frozen across generations: fields
/// may only be added, so any two builds can at least tell each other they are incompatible.
pub const PROTOCOL_VERSION: u32 = 52;

/// Oldest wire generation this build still speaks.
///
/// It trails [`PROTOCOL_VERSION`] by the support window: a build speaks the current generation and
/// the previous one for at least six months after a bump ships. Adapters for the previous
/// generation live in one place and are deleted when the window closes. Generation 52 is the first
/// with the send gate and tolerant decoding, so the window starts there.
pub const MIN_SUPPORTED_PROTOCOL: u32 = 52;

/// `code` on the [`super::ServerMessage::Error`] a client receives just before the server closes it for
/// being evicted. Distinguishes a removal from a dropped connection, which the client would
/// otherwise answer by reconnecting straight back into the session it was removed from.
pub const EVICTED_ERROR_CODE: &str = "evicted";

/// `code` on the [`super::ServerMessage::Error`] a server answers an attach with when the two
/// builds share no protocol version. Every build since protocol negotiation began uses this code,
/// so a client can recognise an older server it cannot otherwise read.
pub const PROTOCOL_MISMATCH_ERROR_CODE: &str = "protocol-mismatch";

/// `code` on the [`super::ServerMessage::Error`] a server answers a request with when it does not
/// know the request's `type`. The connection stays open; only that request failed.
pub const UNSUPPORTED_MESSAGE_ERROR_CODE: &str = "unsupported-message";

pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
pub const PANE_STATUS_MAX_LEN: usize = 64;
pub const PANE_STATUS_REASON_MAX_LEN: usize = 256;
/// How many publishers one client may have in one pane at once. The UI refuses a stream beyond it
/// when the stream opens, so the refusal reaches the publisher; the session server enforces it again
/// per client on the rows it is sent. Clients never share publishers, so no count spans them.
pub const MAX_PANE_PUBLISHERS: usize = 16;

/// On-wire size of a pane byte frame besides the payload: 4-byte length, 1-byte kind, 13-byte
/// header (`pane_id` + `generation` + locality).
pub const PANE_FRAME_OVERHEAD: usize = 18;
pub(crate) const PANE_FRAME_HEADER_LEN: usize = 13;
pub(crate) const FRAME_KIND_CONTROL_JSON: u8 = 1;
pub(crate) const FRAME_KIND_PANE_OUTPUT: u8 = 2;
pub(crate) const FRAME_KIND_PANE_INPUT: u8 = 3;

pub(crate) const FRAME_KIND_PRESENTATION_ACK: u8 = 4;
pub(crate) const FRAME_KIND_PRESENTATION_INTEREST: u8 = 5;
pub(crate) const PANE_OUTPUT_SEMANTICS_LEN: usize = 37;
pub(crate) const PANE_OUTPUT_ACQUISITION_OFFSET: usize =
    PANE_FRAME_OVERHEAD + PANE_OUTPUT_SEMANTICS_LEN;
pub(crate) const PANE_OUTPUT_METADATA_LEN: usize =
    PANE_OUTPUT_SEMANTICS_LEN + size_of::<u64>() + size_of::<u32>();

/// Output frame overhead including semantics and the acquisition header, excluding file bindings.
pub const PANE_OUTPUT_FRAME_OVERHEAD: usize = PANE_FRAME_OVERHEAD + PANE_OUTPUT_METADATA_LEN;

/// Regular-file attachment limits; chunks are base64 inside bounded control frames.
pub const MAX_ATTACHMENT_BYTES: u64 = 16 * 1024 * 1024;
pub const ATTACHMENT_CHUNK_BYTES: usize = 256 * 1024;

pub(crate) const FRAME_KIND_ACQUISITION_ACK: u8 = 6;
