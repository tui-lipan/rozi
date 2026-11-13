use std::collections::{BTreeMap, HashMap};

use crate::git::pull_requests::{Lookup, PullRequestStatus, WorktreeStatuses};
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

impl AgentPullRequests {
    /// The pull request of the branch checked out at `root` on `host`, when it has one. `root` is
    /// compared by its host's path rules, since one directory can be spelled several ways there.
    pub(crate) fn find(
        &self,
        host: Option<&RemoteTarget>,
        root: &str,
        family: PathFamily,
    ) -> Option<PullRequestStatus> {
        self.hosts
            .get(&host.cloned())?
            .values()
            .flat_map(|statuses| &statuses.checkouts)
            .find(|(path, _)| family.paths_equal(path, root))
            .map(|(_, pull_request)| pull_request.clone())
    }
}
