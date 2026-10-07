//! The extension runtimes this client runs, and the placed processes it supervises through them.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

use super::{HostKey, WorkerBinding, WorkerId};
use crate::extension_runtime::client::RuntimeConnection;

/// Why a placed contribution is not running. Shown as-is; never answered by running it elsewhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unavailable {
    /// `[extension] platforms` excludes the host's operating system.
    UnsupportedPlatform { os: String },
    /// The host's Rozi cannot run an extension runtime.
    RuntimeUnsupported { detail: String },
    /// A program the contribution runs is not on the host's `PATH`.
    MissingExecutable { program: String },
    /// The extension's files could not be staged or verified on the host.
    BundleFailed { detail: String },
    /// The SSH connection for the runtime could not be made or was lost.
    RuntimeUnreachable { detail: String },
    /// An `active-session` contribution with no session on screen.
    NoSession,
    /// The process could not be started for another reason.
    SpawnFailed { detail: String },
}

impl std::fmt::Display for Unavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedPlatform { os } => write!(f, "the extension does not support {os}"),
            Self::RuntimeUnsupported { detail } => f.write_str(detail),
            Self::MissingExecutable { program } => {
                write!(f, "`{program}` is not installed or not on the host's PATH")
            }
            Self::BundleFailed { detail } => {
                write!(f, "the extension's files could not be staged: {detail}")
            }
            Self::RuntimeUnreachable { detail } => {
                write!(f, "the extension runtime is unreachable: {detail}")
            }
            Self::NoSession => f.write_str("no session is attached"),
            Self::SpawnFailed { detail } => write!(f, "could not start: {detail}"),
        }
    }
}

impl Unavailable {
    /// A stable kebab-case name for scripts reading `extensions status --json`.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform { .. } => "unsupported-platform",
            Self::RuntimeUnsupported { .. } => "runtime-unsupported",
            Self::MissingExecutable { .. } => "missing-executable",
            Self::BundleFailed { .. } => "bundle-failed",
            Self::RuntimeUnreachable { .. } => "runtime-unreachable",
            Self::NoSession => "no-session",
            Self::SpawnFailed { .. } => "spawn-failed",
        }
    }
}

impl From<crate::extension_runtime::protocol::SpawnFailure> for Unavailable {
    fn from(failure: crate::extension_runtime::protocol::SpawnFailure) -> Self {
        use crate::extension_runtime::protocol::SpawnFailure;
        match failure {
            SpawnFailure::UnsupportedPlatform { os } => Self::UnsupportedPlatform { os },
            SpawnFailure::MissingExecutable { program } => Self::MissingExecutable { program },
            SpawnFailure::BundleMissing => Self::BundleFailed {
                detail: "the files were not staged".to_string(),
            },
            SpawnFailure::BundleCorrupt { detail } => Self::BundleFailed { detail },
            SpawnFailure::SpawnFailed { detail } => Self::SpawnFailed { detail },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostRuntimeStatus {
    Connecting,
    Ready { os: String, version: Option<String> },
    Unavailable(Unavailable),
}

/// One runtime: this machine's in-process one, or one over SSH to a remote host.
pub struct HostRuntime {
    pub host: HostKey,
    /// Identifies this connection. Credentials are issued per epoch, so a reconnect invalidates
    /// every one issued over the old connection.
    pub epoch: u64,
    pub status: HostRuntimeStatus,
    pub connection: Option<Arc<RuntimeConnection>>,
    /// Bundle digests the runtime has confirmed staged.
    pub staged: HashSet<String>,
    /// Bundle digests sent and awaiting confirmation.
    pub staging: HashSet<String>,
    /// Consecutive failed connections, for the reconnect backoff.
    pub failures: u32,
    /// When a lost connection may be retried.
    pub retry_at: Option<Instant>,
}

/// What a placed worker is waiting on before it can be started.
#[derive(Clone, Debug)]
pub struct PendingLaunch {
    pub worker: WorkerId,
    pub digest: String,
    pub message: crate::extension_runtime::protocol::Message,
    /// Restaged once already after the runtime reported the bundle missing or corrupt.
    pub restaged: bool,
    /// The files to stage if the runtime does not have them.
    pub bundle: crate::extension_runtime::bundle::SharedBundle,
}

/// What a placed process is for, which decides what its exit means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerPurpose {
    /// An instance of a placed service; the supervisor decides whether it restarts.
    Service { key: InstanceKey },
    /// One run of a placed command; a failure raises a toast.
    Command { label: String },
    /// One listing of a placed sidebar tab.
    TabListing {
        tab: crate::config::SidebarTabId,
        epoch: u64,
        group_prefix: Option<String>,
    },
    /// A placed tab's `on_click` `exec`.
    TabClick { label: String },
}

#[derive(Clone, Debug)]
pub struct WorkerProcess {
    pub host: HostKey,
    pub purpose: WorkerPurpose,
    pub digest: String,
    pub state: WorkerProcessState,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerProcessState {
    /// Waiting for the runtime to connect or the bundle to stage.
    Pending,
    Starting,
    Running {
        pid: u32,
    },
}

/// One instance of a placed service: which service, and the binding it was started for.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct InstanceKey {
    pub service: String,
    pub binding: WorkerBinding,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstanceStatus {
    Running {
        worker: WorkerId,
    },
    /// Waiting out the restart backoff.
    Restarting {
        at: Instant,
    },
    /// Exited and not restarting under its policy, or gave up after repeated failures.
    Stopped {
        detail: String,
    },
    Unavailable(Unavailable),
}

#[derive(Clone, Debug)]
pub struct PlacedInstance {
    pub key: InstanceKey,
    pub extension: String,
    pub generation: String,
    pub status: InstanceStatus,
    pub started_at: Option<Instant>,
    pub failures: u32,
    pub backoff: Duration,
    /// The reason last shown to the user, so a host that stays unreachable across reconnect
    /// attempts is reported once rather than on every attempt.
    pub reported: Option<&'static str>,
}

#[derive(Default)]
pub struct ExtensionRuntimeState {
    pub hosts: HashMap<HostKey, HostRuntime>,
    pub next_epoch: u64,
    pub processes: HashMap<WorkerId, WorkerProcess>,
    pub pending: Vec<PendingLaunch>,
    /// The spawn message last sent for each live worker, so a restage can send it again.
    pub sent: HashMap<WorkerId, crate::extension_runtime::protocol::Message>,
    /// Workers already restaged once; a second missing or corrupt bundle is believed.
    pub restaged: HashSet<WorkerId>,
    pub instances: Vec<PlacedInstance>,
    /// What the last reconcile saw, so the per-message sync is a comparison when nothing changed.
    pub signature: Option<u64>,
    /// A restart timer or retry is armed for this instant.
    pub wake_at: Option<Instant>,
}
