//! The handoff document: what the outgoing client tells the build that replaces it.
//!
//! It carries the client's identity, not its momentary UI: which sessions it was attached to and
//! how, which one was on screen, where each session's focus was, and the claim on each slot its
//! servers hold. An open palette, a selection, or a toast does not survive a relaunch.
//!
//! The document is written to the private runtime directory, named by a random id that only the
//! command line of the new process carries, and deleted by the first process that reads it. It
//! holds claim tokens, so it lives no longer than the relaunch that needs it.

use std::io::{self, Read, Write};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::plan::Transition;
use crate::pane::id::PaneId;
use crate::platform::paths::PlatformEnv;

/// The layout this build writes. A reader accepts every older schema and refuses a newer one,
/// which only a newer build relaunching into an older one would write.
pub const SCHEMA: u32 = 1;

/// The bytes read from a document before it is refused as not one of ours.
const MAX_DOCUMENT_BYTES: u64 = 1024 * 1024;

/// Hex digits in a document id.
const ID_LEN: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientHandoff {
    pub schema: u32,
    /// The release that wrote the document.
    pub from_release: String,
    /// The session on screen, as an index into `sessions`. `None` for the launcher.
    #[serde(default)]
    pub foreground: Option<usize>,
    /// Every session the client was attached to.
    #[serde(default)]
    pub sessions: Vec<HandoffSession>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandoffSession {
    pub name: String,
    /// The host, as [`crate::session::remote::RemoteTarget::to_spec`] writes it; `None` for this
    /// machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    /// The server instance the client was attached to, to tell a session that was restarted
    /// meanwhile from the one it left.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<String>,
    pub transition: Transition,
    /// The claim on this client's slot, for a [`Transition::Carry`] whose server holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(default)]
    pub read_only: bool,
    /// Created for the user rather than by them (see [`crate::state::Attachment::auto_created`]).
    #[serde(default)]
    pub auto_created: bool,
    /// Worked in (see [`crate::state::Attachment::engaged`]).
    #[serde(default)]
    pub engaged: bool,
    #[serde(default)]
    pub active_workspace: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_pane: Option<PaneId>,
    /// When it was last parked (see [`crate::state::Attachment::parked_seq`]); `0` for one never
    /// parked.
    #[serde(default)]
    pub parked_seq: u64,
}

impl ClientHandoff {
    pub fn new(foreground: Option<usize>, sessions: Vec<HandoffSession>) -> Self {
        Self {
            schema: SCHEMA,
            from_release: env!("CARGO_PKG_VERSION").to_string(),
            foreground,
            sessions,
        }
    }

    /// Drop the session at `index`, which could not be prepared for the new build. The session on
    /// screen keeps its place, or the client comes back on the launcher when it was that one.
    pub fn remove(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        self.sessions.remove(index);
        self.foreground = match self.foreground {
            Some(foreground) if foreground == index => None,
            Some(foreground) if foreground > index => Some(foreground - 1),
            other => other,
        };
    }

    /// The order to attach in so the client ends up as it was: background sessions from the one
    /// parked longest ago, then the session on screen. Attaching parks the previous session, so
    /// this rebuilds both the background set and its recency.
    pub fn restore_order(&self) -> Vec<usize> {
        let mut background: Vec<usize> = (0..self.sessions.len())
            .filter(|index| Some(*index) != self.foreground)
            .collect();
        background.sort_by_key(|index| self.sessions[*index].parked_seq);
        background.extend(self.foreground);
        background
    }
}

fn directory(env: &PlatformEnv) -> io::Result<PathBuf> {
    let dir = crate::platform::paths::runtime_dir(env)?.join("handoffs");
    crate::platform::fs_security::ensure_private_dir(&dir)?;
    Ok(dir)
}

fn valid_id(id: &str) -> bool {
    id.len() == ID_LEN && id.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Write `handoff` where only this user can read it, and return the id that names it.
pub fn store(handoff: &ClientHandoff) -> io::Result<String> {
    store_in(&directory(&PlatformEnv::from_process())?, handoff)
}

fn store_in(dir: &std::path::Path, handoff: &ClientHandoff) -> io::Result<String> {
    let mut random = [0u8; ID_LEN / 2];
    getrandom::fill(&mut random).map_err(io::Error::other)?;
    let id: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
    let bytes = serde_json::to_vec(handoff).map_err(io::Error::other)?;
    let mut file = crate::platform::persist::create_private_file(&dir.join(&id), false)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(id)
}

/// Delete the document `id` names, for a relaunch that did not go ahead.
pub fn discard(id: &str) {
    if valid_id(id)
        && let Ok(dir) = directory(&PlatformEnv::from_process())
    {
        let _ = std::fs::remove_file(dir.join(id));
    }
}

/// Read and delete the document `id` names. It is deleted even when it cannot be used, so a claim
/// token never outlives the one attempt to use it.
pub fn take(id: &str) -> Result<ClientHandoff, String> {
    let dir = directory(&PlatformEnv::from_process()).map_err(|error| error.to_string())?;
    take_from(&dir, id)
}

fn take_from(dir: &std::path::Path, id: &str) -> Result<ClientHandoff, String> {
    if !valid_id(id) {
        return Err("not a handoff id".to_string());
    }
    let path = dir.join(id);
    let mut bytes = Vec::new();
    let read = std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_DOCUMENT_BYTES).read_to_end(&mut bytes));
    let _ = std::fs::remove_file(&path);
    read.map_err(|error| format!("the handoff could not be read: {error}"))?;
    let handoff: ClientHandoff = serde_json::from_slice(&bytes)
        .map_err(|error| format!("the handoff is damaged: {error}"))?;
    if handoff.schema > SCHEMA {
        return Err(format!(
            "the handoff was written by rozi {}, which is newer than this one",
            handoff.from_release
        ));
    }
    if handoff
        .foreground
        .is_some_and(|index| index >= handoff.sessions.len())
    {
        return Err("the handoff names a session it does not hold".to_string());
    }
    Ok(handoff)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(name: &str, parked_seq: u64) -> HandoffSession {
        HandoffSession {
            name: name.to_string(),
            remote: None,
            instance: None,
            transition: Transition::Carry { claim: true },
            token: Some("ab".repeat(32)),
            read_only: false,
            auto_created: false,
            engaged: true,
            active_workspace: 2,
            focused_pane: Some(4),
            parked_seq,
        }
    }

    #[test]
    fn the_session_on_screen_is_attached_last_and_the_rest_by_recency() {
        let handoff = ClientHandoff::new(
            Some(1),
            vec![
                session("recent", 5),
                session("dev", 0),
                session("oldest", 2),
            ],
        );
        assert_eq!(handoff.restore_order(), vec![2, 0, 1]);
        let launcher = ClientHandoff::new(None, vec![session("a", 3), session("b", 1)]);
        assert_eq!(launcher.restore_order(), vec![1, 0]);
    }

    #[test]
    fn removing_a_session_keeps_the_one_on_screen_in_place() {
        let mut handoff = ClientHandoff::new(
            Some(2),
            vec![session("a", 1), session("b", 2), session("dev", 0)],
        );
        handoff.remove(0);
        assert_eq!(handoff.foreground, Some(1));
        assert_eq!(handoff.sessions[1].name, "dev");
        handoff.remove(1);
        assert_eq!(handoff.foreground, None, "the launcher stands in");
        handoff.remove(5);
        assert_eq!(handoff.sessions.len(), 1);
    }

    #[test]
    fn a_discarded_document_cannot_be_read() {
        let handoff = ClientHandoff::new(None, vec![session("dev", 0)]);
        let id = store(&handoff).unwrap();
        discard(&id);
        assert!(take(&id).is_err());
        discard("../not-an-id");
    }

    #[test]
    fn a_document_is_read_once_and_then_gone() {
        let dir = crate::test_support::private_temp_dir("handoff-document");
        let handoff = ClientHandoff::new(Some(0), vec![session("dev", 0)]);
        let id = store_in(&dir, &handoff).unwrap();
        assert!(valid_id(&id));

        assert_eq!(take_from(&dir, &id).unwrap(), handoff);
        assert!(take_from(&dir, &id).is_err(), "the claim tokens are spent");
        assert!(!dir.join(&id).exists());
    }

    #[test]
    fn a_document_from_a_newer_build_or_out_of_shape_is_refused_and_removed() {
        let dir = crate::test_support::private_temp_dir("handoff-refused");
        let newer = ClientHandoff {
            schema: SCHEMA + 1,
            ..ClientHandoff::new(None, Vec::new())
        };
        let id = store_in(&dir, &newer).unwrap();
        assert!(take_from(&dir, &id).unwrap_err().contains("newer"));
        assert!(!dir.join(&id).exists());

        let dangling = ClientHandoff::new(Some(3), vec![session("dev", 0)]);
        let id = store_in(&dir, &dangling).unwrap();
        assert!(take_from(&dir, &id).is_err());

        assert!(take_from(&dir, "../../etc/passwd").is_err());
    }
}
