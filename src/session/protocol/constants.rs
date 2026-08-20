/// Maximum wire protocol version this build speaks.
///
/// Every peer is built from this same tree, so there is one version and no skew to straddle. Bump
/// this for any wire change, additive or not; a mismatched peer is rejected at the handshake rather
/// than shimmed. Per-message capability gates are not worth keeping while that holds — a gate below
/// the floor is a branch that cannot be taken.
pub const PROTOCOL_VERSION: u32 = 3;

/// Oldest wire protocol version this build can still speak.
pub const MIN_SUPPORTED_PROTOCOL: u32 = PROTOCOL_VERSION;

/// `code` on the [`super::ServerMessage::Error`] a client receives just before the server closes it for
/// being evicted. Distinguishes a removal from a dropped connection, which the client would
/// otherwise answer by reconnecting straight back into the session it was removed from.
pub const EVICTED_ERROR_CODE: &str = "evicted";

pub const MAX_FRAME_SIZE: usize = 8 * 1024 * 1024;
pub const PANE_STATUS_MAX_LEN: usize = 64;
pub const PANE_STATUS_REASON_MAX_LEN: usize = 256;

/// On-wire size of a pane byte frame besides the payload: 4-byte length, 1-byte kind, 13-byte
/// header (`pane_id` + `generation` + locality).
pub const PANE_FRAME_OVERHEAD: usize = 18;
pub(crate) const PANE_FRAME_HEADER_LEN: usize = 13;
pub(crate) const FRAME_KIND_CONTROL_JSON: u8 = 1;
pub(crate) const FRAME_KIND_PANE_OUTPUT: u8 = 2;
pub(crate) const FRAME_KIND_PANE_INPUT: u8 = 3;
