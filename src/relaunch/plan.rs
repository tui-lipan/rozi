//! What a relaunch does to each session the client is attached to.
//!
//! The question is asked per session, of the generation that session's server actually agreed to,
//! against the generations the new build speaks. A release that drops an old generation strands
//! only the sessions still on it; every other session carries on untouched.

use std::ops::RangeInclusive;

use serde::{Deserialize, Serialize};

use crate::session::remote::RemoteTarget;

/// What a client knows about one of its sessions when it plans a relaunch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFacts {
    pub name: String,
    pub remote: Option<RemoteTarget>,
    /// The generation its server agreed to on this connection, `None` while it is offline.
    pub generation: Option<u32>,
    /// Whether its server can hold this client's slot across the relaunch.
    pub holds_slot: bool,
    pub read_only: bool,
    /// Whether rozi may be installed on its host, for a remote session that needs a newer one.
    pub remote_install_allowed: bool,
}

impl SessionFacts {
    pub fn ephemeral(&self) -> bool {
        crate::session::naming::is_ephemeral_session_name(&self.name)
    }

    /// `name` or `name@host`, the way every other surface names a session.
    pub fn label(&self) -> String {
        match &self.remote {
            Some(remote) => format!("{}@{}", self.name, remote.display_label()),
            None => self.name.clone(),
        }
    }
}

/// What the relaunch does to one session.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Transition {
    /// The new build attaches to the server that is running now; nothing in the session changes.
    /// With `claim`, it takes over this client's slot, keeping layout control where it was.
    Carry { claim: bool },
    /// The session is offline now. The new build reconnects to it as this one would have.
    Reconnect,
    /// The new build cannot attach to its server, so it restarts the session on itself from its
    /// snapshot first. The programs running in it end.
    Upgrade,
    /// It cannot be carried over, for the reason given. Nothing happens until it is dealt with.
    Blocked { reason: String },
}

impl Transition {
    /// Whether this ends programs that are running now.
    pub fn is_destructive(&self) -> bool {
        matches!(self, Self::Upgrade)
    }
}

/// Decide what relaunching onto a build that speaks `generations` does to the session `facts`
/// describes.
pub fn transition(facts: &SessionFacts, generations: &RangeInclusive<u32>) -> Transition {
    let Some(generation) = facts.generation else {
        return Transition::Reconnect;
    };
    if generations.contains(&generation) {
        return Transition::Carry {
            claim: facts.holds_slot,
        };
    }
    if generation > *generations.end() {
        return Transition::Blocked {
            reason: "runs a newer rozi than the one installed".to_string(),
        };
    }
    if facts.read_only {
        return Transition::Blocked {
            reason: "needs an upgrade, which a viewer cannot start".to_string(),
        };
    }
    if facts.ephemeral() {
        return Transition::Blocked {
            reason: "needs an upgrade; name it first, a temporary session has no snapshot"
                .to_string(),
        };
    }
    if facts.remote.is_some() && !facts.remote_install_allowed {
        return Transition::Blocked {
            reason: "needs a newer rozi on its host, and [remote] install is \"never\"".to_string(),
        };
    }
    Transition::Upgrade
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(name: &str, generation: Option<u32>) -> SessionFacts {
        SessionFacts {
            name: name.to_string(),
            remote: None,
            generation,
            holds_slot: true,
            read_only: false,
            remote_install_allowed: true,
        }
    }

    #[test]
    fn a_session_the_new_build_speaks_to_is_carried_over_untouched() {
        assert_eq!(
            transition(&facts("dev", Some(52)), &(52..=53)),
            Transition::Carry { claim: true }
        );
        let older_server = SessionFacts {
            holds_slot: false,
            ..facts("dev", Some(52))
        };
        assert_eq!(
            transition(&older_server, &(52..=53)),
            Transition::Carry { claim: false },
            "a server without the capability is reattached without a claim"
        );
        assert_eq!(
            transition(&facts("dev", None), &(53..=53)),
            Transition::Reconnect
        );
    }

    #[test]
    fn only_a_session_on_a_dropped_generation_is_upgraded() {
        let upgrade = transition(&facts("backend", Some(51)), &(52..=53));
        assert_eq!(upgrade, Transition::Upgrade);
        assert!(upgrade.is_destructive());

        let remote = SessionFacts {
            remote: Some(RemoteTarget::Alias("workbox".into())),
            ..facts("api", Some(51))
        };
        assert_eq!(transition(&remote, &(52..=53)), Transition::Upgrade);
        assert_eq!(remote.label(), "api@workbox");
    }

    #[test]
    fn what_cannot_be_upgraded_from_here_blocks_the_relaunch_with_a_reason() {
        let generations = 52..=53;
        let cases = [
            facts("dev", Some(54)),
            SessionFacts {
                read_only: true,
                ..facts("shared", Some(51))
            },
            facts("eph-0123456789abcdef01234567", Some(51)),
            SessionFacts {
                remote: Some(RemoteTarget::Alias("workbox".into())),
                remote_install_allowed: false,
                ..facts("api", Some(51))
            },
        ];
        for case in cases {
            assert!(
                matches!(transition(&case, &generations), Transition::Blocked { .. }),
                "{case:?}"
            );
        }
    }
}
