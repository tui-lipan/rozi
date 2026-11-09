//! What a relaunch does to each session the client is attached to.
//!
//! The question is asked per session, of the server itself: every generation it speaks and the
//! release it runs, against the generations and release of the build about to run. Whether this
//! client's connection happened to agree on an older generation does not matter, since the new
//! build agrees on its own; only a server the new build shares no generation with needs
//! replacing, and then the same rule decides as for `rozi sessions upgrade`, measured against the
//! new build rather than this one.

use std::ops::RangeInclusive;

use semver::Version;
use serde::{Deserialize, Serialize};

use crate::session::discovery::{ReplacementTarget, SessionRelease, UpgradeDisposition};
use crate::session::protocol::PeerRelease;
use crate::session::remote::RemoteTarget;

/// What a client knows about one of its sessions when it plans a relaunch.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionFacts {
    pub name: String,
    pub remote: Option<RemoteTarget>,
    /// The server, as this client's connection found it; `None` while the session is offline.
    pub server: Option<ServerFacts>,
    /// Whether its server can hold this client's slot across the relaunch.
    pub holds_slot: bool,
    pub read_only: bool,
    /// Whether rozi may be installed on its host, for a remote session that needs a newer one.
    pub remote_install_allowed: bool,
}

/// What a session's server said about itself when this client attached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerFacts {
    /// Every session wire generation it speaks.
    pub protocol: RangeInclusive<u32>,
    /// Its release, when it said.
    pub release: Option<String>,
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
    /// The new build shares no generation with its server, which runs an older rozi, so it restarts
    /// the session on itself from its snapshot. The programs running in it end.
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

/// Decide what relaunching onto the build `release`, speaking `generations`, does to the session
/// `facts` describes.
pub fn transition(
    facts: &SessionFacts,
    release: &Version,
    generations: &RangeInclusive<u32>,
) -> Transition {
    let Some(server) = &facts.server else {
        return Transition::Reconnect;
    };
    let shared = server.protocol.start().max(generations.start())
        <= server.protocol.end().min(generations.end());
    if shared {
        return Transition::Carry {
            claim: facts.holds_slot,
        };
    }
    let target = ReplacementTarget {
        release: release.clone(),
        protocol: generations.clone(),
    };
    let session = match &server.release {
        Some(release) => SessionRelease::Incompatible(Some(PeerRelease {
            rozi_version: release.clone(),
            protocol_min: *server.protocol.start(),
            protocol_max: *server.protocol.end(),
        })),
        None => SessionRelease::Unknown,
    };
    let blocked = |reason: String| Transition::Blocked { reason };
    match session.upgrade_disposition_onto(&target) {
        UpgradeDisposition::RefuseNewer(newer) => {
            return blocked(format!("runs rozi {newer}, newer than the one installed"));
        }
        UpgradeDisposition::RefuseUnverified => {
            return blocked("cannot tell whether it runs an older rozi".to_string());
        }
        UpgradeDisposition::Replace => {}
    }
    if facts.read_only {
        return blocked("needs an upgrade, which a viewer cannot start".to_string());
    }
    if facts.ephemeral() {
        return blocked(
            "needs an upgrade; name it first, a temporary session has no snapshot".to_string(),
        );
    }
    if facts.remote.is_some() && !facts.remote_install_allowed {
        return blocked(
            "needs a newer rozi on its host, and [remote] install is \"never\"".to_string(),
        );
    }
    Transition::Upgrade
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    fn facts(name: &str, protocol: RangeInclusive<u32>, release: &str) -> SessionFacts {
        SessionFacts {
            name: name.to_string(),
            remote: None,
            server: Some(ServerFacts {
                protocol,
                release: Some(release.to_string()),
            }),
            holds_slot: true,
            read_only: false,
            remote_install_allowed: true,
        }
    }

    /// The new build is 0.0.50, speaking only generation 53.
    fn plan(facts: &SessionFacts) -> Transition {
        transition(facts, &version("0.0.50"), &(53..=53))
    }

    #[test]
    fn a_server_the_new_build_shares_a_generation_with_is_carried_over_untouched() {
        // This client agreed on 52 with a server that also speaks 53: the new build agrees on 53.
        assert_eq!(
            plan(&facts("dev", 52..=53, "0.0.45")),
            Transition::Carry { claim: true }
        );
        let older_server = SessionFacts {
            holds_slot: false,
            ..facts("dev", 53..=53, "0.0.49")
        };
        assert_eq!(
            plan(&older_server),
            Transition::Carry { claim: false },
            "a server without the capability is reattached without a claim"
        );
        // A newer release that still speaks 53 is carried over too, never refused.
        assert_eq!(
            plan(&facts("dev", 53..=54, "0.0.60")),
            Transition::Carry { claim: true }
        );
        let offline = SessionFacts {
            server: None,
            ..facts("dev", 53..=53, "0.0.50")
        };
        assert_eq!(plan(&offline), Transition::Reconnect);
    }

    #[test]
    fn only_a_server_on_an_older_release_and_older_generations_is_upgraded() {
        let upgrade = plan(&facts("backend", 51..=52, "0.0.44"));
        assert_eq!(upgrade, Transition::Upgrade);
        assert!(upgrade.is_destructive());

        let remote = SessionFacts {
            remote: Some(RemoteTarget::Alias("workbox".into())),
            ..facts("api", 51..=52, "0.0.44")
        };
        assert_eq!(plan(&remote), Transition::Upgrade);
        assert_eq!(remote.label(), "api@workbox");
    }

    #[test]
    fn what_cannot_be_upgraded_from_here_blocks_the_relaunch_with_a_reason() {
        let cases = [
            // Newer generations than the new build speaks.
            facts("ahead", 54..=55, "0.0.70"),
            // A newer release, whatever its generations.
            facts("newer", 50..=52, "0.0.60"),
            // A release that cannot be read.
            facts("odd", 50..=52, "not-a-version"),
            SessionFacts {
                server: Some(ServerFacts {
                    protocol: 50..=52,
                    release: None,
                }),
                ..facts("silent", 50..=52, "0.0.1")
            },
            SessionFacts {
                read_only: true,
                ..facts("shared", 51..=52, "0.0.44")
            },
            facts("eph-0123456789abcdef01234567", 51..=52, "0.0.44"),
            SessionFacts {
                remote: Some(RemoteTarget::Alias("workbox".into())),
                remote_install_allowed: false,
                ..facts("api", 51..=52, "0.0.44")
            },
        ];
        for case in cases {
            assert!(
                matches!(plan(&case), Transition::Blocked { .. }),
                "{case:?} -> {:?}",
                plan(&case)
            );
        }
    }
}
