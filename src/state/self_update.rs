//! **Update rozi** while it runs: the release it installs, how far it has got, and whether its card
//! is on screen.

use semver::Version;

/// How many lines of a package manager's output the card keeps, newest last.
pub const OUTPUT_LINES: usize = 4;

#[derive(Debug, Default)]
pub struct SelfUpdateState {
    /// The update this client is running, or ran and is still showing.
    pub run: Option<SelfUpdateRun>,
    /// Fences what an earlier run's worker still sends after a newer run started.
    pub generation: u64,
}

impl SelfUpdateState {
    /// Whether the card is on screen.
    pub fn visible(&self) -> bool {
        self.run.as_ref().is_some_and(|run| !run.hidden)
    }
}

/// How this install takes a new release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateChannel {
    /// rozi's own installer, run in this process: a real download, then the switch.
    Managed,
    /// The package manager that owns this install, run as `command`. `source` is that channel,
    /// which is where the update looks for what it installed, and nowhere else.
    Command {
        command: &'static str,
        source: crate::platform::install_source::InstallSource,
    },
}

#[derive(Debug)]
pub struct SelfUpdateRun {
    pub generation: u64,
    pub running: Version,
    /// The release the check announced, which the update set out to install. What it actually
    /// installed is [`Self::installed`]: a newer release can be published in between.
    pub latest: Version,
    /// The release-wide step [`Self::latest`]'s metadata raised, such as a new extension API.
    /// Carried by the run rather than looked up later, so a newer release announced meanwhile
    /// cannot lend this one its warning.
    pub note: Option<String>,
    pub channel: UpdateChannel,
    pub phase: UpdatePhase,
    /// The card was put away while the update kept going. It comes back through **Update rozi**,
    /// and a toast says when the update is done.
    pub hidden: bool,
    /// The newest lines a package manager printed, for the card to show under its row.
    pub output: std::collections::VecDeque<String>,
    /// The size of the release once it has fully arrived, for the finished download row.
    pub downloaded: Option<u64>,
    /// The release the update proved it installed: what the installer activated, or what the
    /// package manager's rozi says it is. Set once the update succeeds.
    pub installed: Option<Version>,
    /// The hold on a package manager's command, which this client ends if it leaves first.
    pub stop: Option<std::sync::Arc<crate::platform::command::CommandStop>>,
}

impl SelfUpdateRun {
    /// The release the card names: the one installed once it is known, the announced one before.
    pub fn release(&self) -> &Version {
        self.installed.as_ref().unwrap_or(&self.latest)
    }

    /// The release-wide note for `version`, which only the announced release has.
    pub fn note_for(&self, version: &Version) -> Option<String> {
        (*version == self.latest)
            .then(|| self.note.clone())
            .flatten()
    }
}

/// A step of an update, for saying which one stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpdateStep {
    Download,
    Install,
    /// A package manager's command, which is the whole update.
    Command,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UpdatePhase {
    /// Fetching the release. `total` is `None` until the server says how large it is.
    Downloading { downloaded: u64, total: Option<u64> },
    /// The release is here; it is being verified and made the installed one.
    Installing,
    /// A package manager is doing the whole update itself.
    Running,
    /// The new release is installed. The card shows it for a moment before it becomes the
    /// restart dialog.
    Installed,
    /// The update stopped at `step`, and why.
    Failed { step: UpdateStep, reason: String },
}

impl UpdatePhase {
    /// Whether the update is still working, as opposed to settled either way.
    pub fn in_progress(&self) -> bool {
        matches!(
            self,
            Self::Downloading { .. } | Self::Installing | Self::Running
        )
    }

    /// The step this phase is in, or stopped at.
    pub fn step(&self) -> UpdateStep {
        match self {
            Self::Downloading { .. } => UpdateStep::Download,
            Self::Installing | Self::Installed => UpdateStep::Install,
            Self::Running => UpdateStep::Command,
            Self::Failed { step, .. } => *step,
        }
    }

    /// Stop here, for `reason`.
    pub fn fail(&mut self, reason: String) {
        *self = Self::Failed {
            step: self.step(),
            reason,
        };
    }
}

/// What an update's worker reports while it runs.
#[derive(Clone, Debug, PartialEq)]
pub enum SelfUpdateEvent {
    Downloaded {
        downloaded: u64,
        total: Option<u64>,
    },
    /// The release archive, `bytes` long, arrived in full and passed verification: the download
    /// is over, whether or not its length was known, and installing begins.
    ArchiveReady {
        bytes: u64,
    },
    /// Complete lines a package manager printed since the last report.
    Output(Vec<String>),
}

impl SelfUpdateRun {
    /// Apply a worker's report. Output is kept to the last [`OUTPUT_LINES`].
    pub fn advance(&mut self, event: SelfUpdateEvent) {
        if !self.phase.in_progress() {
            return;
        }
        match event {
            SelfUpdateEvent::Downloaded { downloaded, total } => {
                self.phase = UpdatePhase::Downloading { downloaded, total };
            }
            SelfUpdateEvent::ArchiveReady { bytes } => {
                self.downloaded = Some(bytes);
                self.phase = UpdatePhase::Installing;
            }
            SelfUpdateEvent::Output(lines) => {
                self.output.extend(lines);
                while self.output.len() > OUTPUT_LINES {
                    self.output.pop_front();
                }
            }
        }
    }
}
