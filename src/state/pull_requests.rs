use std::collections::{BTreeMap, HashMap};

use crate::git::pull_requests::{Lookup, WorkStatus, WorktreeStatuses};
use crate::platform::paths::PathFamily;
use crate::session::remote::RemoteTarget;

/// The pull requests of the checkouts agents work in, on every host, looked up only while the
/// Agents view or the sidebar Agents tab is on screen. Each host looks up its own repositories:
/// this machine through [`Self::local`], another through its monitor. See `ops::pull_requests`.
#[derive(Default)]
pub struct AgentPullRequests {
    /// This machine's lookup, started the first time one of its repositories is wanted.
    pub(crate) local: Option<Lookup>,
    /// What each host found, by the repository path it was asked about. `None` is this machine.
    pub(crate) hosts: HashMap<Option<RemoteTarget>, BTreeMap<String, WorktreeStatuses>>,
    pub(crate) tick_armed: bool,
}

/// A checkout's pull request as an agent row shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentPullRequest {
    pub number: u64,
    pub status: WorkStatus,
    /// The host's last lookup failed, so this is what the one before it found, and the status may
    /// have moved on since.
    pub stale: bool,
}

impl AgentPullRequests {
    /// The pull request of the branch checked out at `root` on `host`, when it has one. `root` is
    /// compared by its host's path rules, since one directory can be spelled several ways there.
    pub(crate) fn find(
        &self,
        host: Option<&RemoteTarget>,
        root: &str,
        family: PathFamily,
    ) -> Option<AgentPullRequest> {
        self.hosts
            .get(&host.cloned())?
            .values()
            .find_map(|statuses| {
                statuses
                    .checkouts
                    .iter()
                    .find(|(path, _)| family.paths_equal(path, root))
                    .map(|(_, pull_request)| AgentPullRequest {
                        number: pull_request.number,
                        status: pull_request.status,
                        stale: statuses.unavailable,
                    })
            })
    }
}
