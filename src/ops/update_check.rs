//! Quiet update checks, the notice they raise, and the compatibility caution derived from release
//! metadata.

use relswap::{Downloader, UreqDownloader};
use semver::Version;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;
use tui_lipan::prelude::CommandLink;
use url::Url;

use crate::Msg;
use crate::config::EXTENSION_API_VERSION;
use crate::platform::install_source::InstallSource;
use crate::release_app::ROZI;
use crate::session::protocol::PROTOCOL_VERSION;

const COMPATIBILITY_SCHEMA_VERSION: u32 = 1;
const COMPATIBILITY_FILE: &str = "rozi-compatibility.json";
const MAX_COMPATIBILITY_BYTES: usize = 16 * 1024;

/// A public release newer than the running build, and what it takes to move to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub(crate) latest: Version,
    running: Version,
    source: InstallSource,
    compatibility: Option<ReleaseCompatibility>,
}

impl AvailableUpdate {
    /// The command that installs the release, when this install has one rozi can name: its own
    /// updater for a managed install, the package manager's upgrade for a known channel. `None` for
    /// a distribution package or an unrecognised layout, where no single command is right.
    pub(crate) fn update_command(&self) -> Option<&'static str> {
        match self.source {
            InstallSource::Managed => Some("rozi update"),
            source => source.upgrade_command(),
        }
    }

    pub(crate) fn toast_title(&self) -> String {
        format!("rozi v{} available", self.latest)
    }

    /// Terse rows rather than a paragraph: the version step, how to take it, and - only when the
    /// release moves a contract - what that costs.
    ///
    /// `in_commands` says whether this client offers **Update rozi** in Commands, which is the
    /// only way to find the notice again once the toast is gone.
    pub(crate) fn toast_body(&self, in_commands: bool) -> String {
        let how = match (self.update_command(), self.source) {
            (Some(command), _) if in_commands => {
                format!("run `{command}` or use command Update rozi")
            }
            (Some(command), _) => format!("run `{command}`"),
            (None, InstallSource::SystemPackage) => "update with your package manager".to_string(),
            (None, _) => format!("see {}/installation", env!("CARGO_PKG_HOMEPAGE")),
        };
        let mut lines = vec![format!("v{} → v{}", self.running, self.latest), how];
        lines.extend(self.compatibility_note());
        lines.join("\n")
    }

    /// What is left once the release is installed: this client runs the old build until it is
    /// started again, and a protocol change also needs the session servers restarted.
    pub(crate) fn installed_body(&self) -> String {
        let mut lines = vec!["quit and start rozi again to use it".to_string()];
        lines.extend(self.compatibility_note());
        lines.join("\n")
    }

    /// Whether the release moves a contract the user has to act on, which is what makes the notice
    /// a warning rather than news.
    pub(crate) fn needs_caution(&self) -> bool {
        self.compatibility_note().is_some()
    }

    fn compatibility_note(&self) -> Option<String> {
        let compatibility = self.compatibility.as_ref()?;
        let extension_bump = (compatibility.extension_api > EXTENSION_API_VERSION)
            .then_some(compatibility.extension_api);
        let protocol_bump = (compatibility.session_protocol > PROTOCOL_VERSION)
            .then_some(compatibility.session_protocol);
        let mut parts = Vec::new();
        if let Some(extension_api) = extension_bump {
            parts.push(format!(
                "extension API {EXTENSION_API_VERSION}→{extension_api}"
            ));
        }
        if let Some(protocol) = protocol_bump {
            parts.push(format!("session protocol {PROTOCOL_VERSION}→{protocol}"));
        }
        match (extension_bump, protocol_bump) {
            (None, None) => return None,
            (Some(_), None) => parts.push("review extensions first".to_string()),
            (None, Some(_)) => parts.push("restart sessions after".to_string()),
            (Some(_), Some(_)) => {
                parts.push("review extensions, restart sessions after".to_string());
            }
        }
        Some(parts.join(" · "))
    }

    /// An update to `latest` from the running build, optionally moving the extension API and
    /// session protocol to the given versions.
    #[cfg(test)]
    pub(crate) fn for_test(
        latest: &str,
        source: InstallSource,
        contracts: Option<(u32, u32)>,
    ) -> Self {
        let latest = Version::parse(latest).unwrap();
        Self {
            running: Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
            source,
            compatibility: contracts.map(|(extension_api, session_protocol)| {
                ReleaseCompatibility {
                    schema_version: COMPATIBILITY_SCHEMA_VERSION,
                    version: latest.clone(),
                    extension_api,
                    session_protocol,
                }
            }),
            latest,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseCompatibility {
    schema_version: u32,
    version: Version,
    extension_api: u32,
    session_protocol: u32,
}

/// The command **Update rozi** would run from this client right now, or `None` when it has nothing
/// to offer here.
///
/// Nothing when no newer release is known, when this client already ran the update, or when the
/// install has no single command (a distribution package, an unrecognised layout). And nothing
/// while a remote session is in front: the popup runs on that session's server, which would update
/// the host rather than the rozi the user is looking at.
pub(crate) fn update_command_here(state: &crate::state::State) -> Option<&'static str> {
    if state.update_started || state.current().remote_target.is_some() {
        return None;
    }
    state.available_update.as_ref()?.update_command()
}

/// Install the known newer release in a popup. The popup stays open once the updater exits, so its
/// result - success, or the error that stopped it - is there to read.
pub(crate) fn run_update(
    ctx: &mut tui_lipan::prelude::Context<crate::AppRoot>,
) -> tui_lipan::prelude::Update {
    let Some(command) = update_command_here(&ctx.state) else {
        return tui_lipan::prelude::Update::none();
    };
    // Refused before anything is marked: the row must survive a popup that never opened.
    if ctx.state.popup_is_present() {
        return crate::pane::pty_events::notify_info(ctx, "Close the open popup to update rozi")
            .update();
    }
    ctx.state.update_started = true;
    ctx.state.commands_dirty = true;
    crate::ops::user_command::execute(
        ctx,
        &crate::config::UserCommandAction::Popup {
            command: command.to_string(),
            keep_open: true,
        },
    )
}

/// Follow up on the update popup once its updater exits.
///
/// Success installs the release for the next launch only - this client and every session server
/// keep running the build they started with - so the toast names the step that is left. A failure
/// is already on screen in the popup, which stays open; all that changes is that **Update rozi**
/// comes back, so the user can try again without hunting for the command.
///
/// The popup is recognised by the command it runs: only one popup can be open, and [`run_update`]
/// refuses to start while another is.
pub(crate) fn popup_exited(ctx: &mut tui_lipan::prelude::Context<crate::AppRoot>) {
    if !ctx.state.update_started {
        return;
    }
    let (Some(popup), Some(update)) =
        (ctx.state.popup.as_ref(), ctx.state.available_update.clone())
    else {
        return;
    };
    let Some(command) = update.update_command() else {
        return;
    };
    if popup.identity.launch != Some(crate::pane::launch::PaneLaunch::shell(command)) {
        return;
    }
    let tui_lipan::prelude::ManagedTerminalStatus::Exited(code) = popup.terminal.status else {
        return;
    };
    if code != 0 {
        ctx.state.update_started = false;
        ctx.state.commands_dirty = true;
        return;
    }
    crate::pane::pty_events::notify_update(
        ctx,
        format!("rozi v{} installed", update.latest),
        update.installed_body(),
        update.needs_caution(),
    );
}

/// Look for a newer release and report it to this client.
///
/// Every client that finds one is told, so each can offer **Update rozi** in Commands; `announce`
/// is set only for the one client that claimed the toast for this release.
///
/// Runs on the calling worker thread and blocks it for the length of two HTTPS requests, which is
/// why nothing that draws a frame ever calls it directly.
pub(crate) fn announce_available(link: &CommandLink<Msg>) {
    let Some(update) = check() else {
        return;
    };
    let announce = claim_notice(&update.latest);
    link.send(Msg::UpdateAvailable { update, announce });
}

/// Check signed latest-release metadata without delaying the UI.
///
/// The caller runs this on a worker thread. Network and compatibility-metadata failures stay
/// silent: an update toast is useful, but a machine being offline is not an error worth a toast.
pub(crate) fn check() -> Option<AvailableUpdate> {
    let running = Version::parse(env!("CARGO_PKG_VERSION")).ok()?;
    let repository = Url::parse(ROZI.repository_url).ok()?;
    let downloader = UreqDownloader::new();
    let latest = relswap::fetch_latest_metadata(&ROZI, &downloader, &repository)
        .ok()?
        .version;
    if latest <= running {
        return None;
    }

    let compatibility = fetch_compatibility(&downloader, &repository, &latest);
    Some(AvailableUpdate {
        latest,
        running,
        source: crate::platform::install_source::detect_current(),
        compatibility,
    })
}

/// Atomically let one client announce each release. If state storage is unavailable, prefer a
/// repeated useful notice over hiding updates forever.
///
/// This is also what keeps the periodic re-check quiet: every later check of the same release
/// finds the marker and says nothing, so a client left open for a week toasts once.
fn claim_notice(latest: &Version) -> bool {
    let env = crate::platform::paths::PlatformEnv::from_process();
    claim_notice_in(
        &crate::platform::paths::state_dir(&env).join("update-notices"),
        latest,
    )
}

fn claim_notice_in(directory: &Path, latest: &Version) -> bool {
    if crate::platform::fs_security::ensure_private_dir(directory).is_err() {
        return true;
    }
    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(directory.join(format!("v{latest}")))
    {
        Ok(_) => true,
        Err(error) if error.kind() == ErrorKind::AlreadyExists => false,
        Err(_) => true,
    }
}

fn fetch_compatibility(
    downloader: &impl Downloader,
    repository: &Url,
    latest: &Version,
) -> Option<ReleaseCompatibility> {
    let url = repository
        .join(&format!("releases/download/v{latest}/{COMPATIBILITY_FILE}"))
        .ok()?;
    let response = downloader.fetch(&url, MAX_COMPATIBILITY_BYTES).ok()?;
    let document: ReleaseCompatibility = serde_json::from_slice(&response.bytes).ok()?;
    (document.schema_version == COMPATIBILITY_SCHEMA_VERSION && document.version == *latest)
        .then_some(document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use relswap::{DownloadResponse, ReleaseError};

    struct FakeDownloader {
        bytes: Vec<u8>,
    }

    impl Downloader for FakeDownloader {
        fn fetch(
            &self,
            url: &Url,
            _max_bytes: usize,
        ) -> std::result::Result<DownloadResponse, ReleaseError> {
            Ok(DownloadResponse::new(
                url.clone(),
                url.clone(),
                Vec::new(),
                self.bytes.clone(),
            ))
        }
    }

    fn update(source: InstallSource) -> AvailableUpdate {
        AvailableUpdate::for_test("9.0.0", source, None)
    }

    fn compatibility(extension_api: u32, session_protocol: u32) -> AvailableUpdate {
        AvailableUpdate::for_test(
            "9.0.0",
            InstallSource::Managed,
            Some((extension_api, session_protocol)),
        )
    }

    #[test]
    fn compatibility_note_names_each_contract_that_moves_forward() {
        let both = compatibility(EXTENSION_API_VERSION + 1, PROTOCOL_VERSION + 1);
        assert!(both.needs_caution());
        let body = both.toast_body(false);
        let note = body
            .lines()
            .nth(2)
            .expect("a third row carries the caution");
        assert!(note.starts_with("extension API"), "{note}");
        assert!(note.contains("session protocol"), "{note}");
        assert!(note.ends_with("restart sessions after"), "{note}");

        let protocol = compatibility(EXTENSION_API_VERSION, PROTOCOL_VERSION + 1).toast_body(false);
        assert!(protocol.ends_with(&format!(
            "session protocol {PROTOCOL_VERSION}→{} · restart sessions after",
            PROTOCOL_VERSION + 1
        )));

        let unchanged = compatibility(EXTENSION_API_VERSION, PROTOCOL_VERSION);
        assert!(!unchanged.needs_caution());
        assert_eq!(unchanged.toast_body(false).lines().count(), 2);
    }

    /// The body is rows a glance can take in, not a sentence: the step, then what to do.
    #[test]
    fn toast_body_leads_with_the_version_step() {
        let body = update(InstallSource::Managed).toast_body(false);
        let running = env!("CARGO_PKG_VERSION");
        assert_eq!(body, format!("v{running} → v9.0.0\nrun `rozi update`"));
        assert_eq!(
            update(InstallSource::Managed).toast_title(),
            "rozi v9.0.0 available"
        );
    }

    /// Commands is named only where it actually holds the row, so the toast never points at
    /// something that is not there.
    #[test]
    fn toast_body_mentions_commands_only_when_it_can_run_the_update() {
        let body = update(InstallSource::Managed).toast_body(true);
        assert!(body.ends_with("run `rozi update` or use command Update rozi"));
        let body = update(InstallSource::SystemPackage).toast_body(true);
        assert!(body.ends_with("update with your package manager"), "{body}");
        let body = update(InstallSource::Unknown).toast_body(true);
        assert!(body.ends_with("/installation"), "{body}");
    }

    #[test]
    fn compatibility_document_must_match_the_signed_release_version() {
        let repository = Url::parse("https://github.com/tui-lipan/rozi/").unwrap();
        let latest = Version::parse("2.0.0").unwrap();
        let wrong_version = FakeDownloader {
            bytes:
                br#"{"schema_version":1,"version":"1.0.0","extension_api":2,"session_protocol":5}"#
                    .to_vec(),
        };
        assert!(fetch_compatibility(&wrong_version, &repository, &latest).is_none());

        let matching = FakeDownloader {
            bytes:
                br#"{"schema_version":1,"version":"2.0.0","extension_api":2,"session_protocol":5}"#
                    .to_vec(),
        };
        assert_eq!(
            fetch_compatibility(&matching, &repository, &latest)
                .unwrap()
                .version,
            latest
        );
    }

    #[test]
    fn update_command_respects_the_install_owner() {
        assert_eq!(
            update(InstallSource::Managed).update_command(),
            Some("rozi update")
        );
        assert_eq!(
            update(InstallSource::Cargo).update_command(),
            Some("cargo install rozi --locked")
        );
        assert_eq!(update(InstallSource::SystemPackage).update_command(), None);
        assert_eq!(update(InstallSource::Unknown).update_command(), None);
    }

    #[test]
    fn only_one_client_claims_a_release_notice() {
        let root = tempfile::tempdir().unwrap();
        let notices = root.path().join("update-notices");
        let first = Version::parse("2.0.0").unwrap();
        let second = Version::parse("2.0.1").unwrap();

        assert!(claim_notice_in(&notices, &first));
        assert!(!claim_notice_in(&notices, &first));
        assert!(claim_notice_in(&notices, &second));
    }
}
