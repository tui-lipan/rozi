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

/// The `rozi-compatibility.json` layout this build reads. A document with another schema reads as
/// no compatibility information: the notice then names only the version step.
const COMPATIBILITY_SCHEMA_VERSION: u32 = 2;
const COMPATIBILITY_FILE: &str = "rozi-compatibility.json";
const MAX_COMPATIBILITY_BYTES: usize = 16 * 1024;
/// The row for a release that still speaks this build's session protocol generation.
const SESSIONS_KEEP_WORKING_NOTE: &str = "running sessions keep working";

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

    /// Terse rows rather than a paragraph: the version step, how to take it, and - when the
    /// release's contracts are known - what it means for extensions and running sessions.
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
        lines.extend(self.compatibility_notes());
        lines.join("\n")
    }

    /// What is left once the release is installed: this client runs the old build until it is
    /// started again, and sessions outside the release's protocol window need upgrading.
    pub(crate) fn installed_body(&self) -> String {
        let mut lines = vec!["quit and start rozi again to use it".to_string()];
        lines.extend(self.compatibility_notes());
        lines.join("\n")
    }

    /// Whether the release moves a contract the user has to act on, which is what makes the notice
    /// a warning rather than news.
    pub(crate) fn needs_caution(&self) -> bool {
        self.compatibility.as_ref().is_some_and(|compatibility| {
            compatibility.raises_extension_api() || !compatibility.speaks_this_generation()
        })
    }

    /// One row per contract, nothing when the release published none.
    ///
    /// The extension API is a public contract, so its row names the step. The session row says
    /// whether sessions running now - all started on this build's generation or an older one - stay
    /// reachable from the new release. When they do not, **Upgrade session** restarts each one on
    /// the new build, which ends the programs in its panes.
    fn compatibility_notes(&self) -> Vec<String> {
        let Some(compatibility) = self.compatibility.as_ref() else {
            return Vec::new();
        };
        let mut notes = Vec::new();
        if compatibility.raises_extension_api() {
            notes.push(format!(
                "extension API {EXTENSION_API_VERSION}→{} · review extensions first",
                compatibility.extension_api
            ));
        }
        notes.push(if compatibility.speaks_this_generation() {
            SESSIONS_KEEP_WORKING_NOTE.to_string()
        } else {
            format!(
                "sessions on generation {PROTOCOL_VERSION} need Upgrade session · ends their programs"
            )
        });
        notes
    }

    /// An update to `latest` from the running build, optionally moving the extension API and
    /// speaking only the given session protocol generation.
    #[cfg(test)]
    pub(crate) fn for_test(
        latest: &str,
        source: InstallSource,
        contracts: Option<(u32, u32)>,
    ) -> Self {
        Self::for_test_window(
            latest,
            source,
            contracts.map(|(extension_api, generation)| (extension_api, generation, generation)),
        )
    }

    /// An update to `latest` whose release speaks session protocol generations `min..=max`.
    #[cfg(test)]
    pub(crate) fn for_test_window(
        latest: &str,
        source: InstallSource,
        contracts: Option<(u32, u32, u32)>,
    ) -> Self {
        let latest = Version::parse(latest).unwrap();
        Self {
            running: Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
            source,
            compatibility: contracts.map(
                |(extension_api, session_protocol_min, session_protocol_max)| {
                    ReleaseCompatibility {
                        schema_version: COMPATIBILITY_SCHEMA_VERSION,
                        version: latest.clone(),
                        extension_api,
                        session_protocol_min,
                        session_protocol_max,
                    }
                },
            ),
            latest,
        }
    }
}

/// `rozi-compatibility.json`, published next to each release's signed manifest.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ReleaseCompatibility {
    schema_version: u32,
    version: Version,
    extension_api: u32,
    /// The oldest session protocol generation the release accepts from a peer.
    session_protocol_min: u32,
    /// The generation the release speaks.
    session_protocol_max: u32,
}

impl ReleaseCompatibility {
    fn raises_extension_api(&self) -> bool {
        self.extension_api > EXTENSION_API_VERSION
    }

    /// Whether the release still accepts sessions started on this build's generation, so a client
    /// updated to it attaches to them as they are.
    fn speaks_this_generation(&self) -> bool {
        (self.session_protocol_min..=self.session_protocol_max).contains(&PROTOCOL_VERSION)
    }
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

    /// A release with this extension API that speaks session generations `min..=max`.
    fn compatibility(extension_api: u32, min: u32, max: u32) -> AvailableUpdate {
        AvailableUpdate::for_test_window(
            "9.0.0",
            InstallSource::Managed,
            Some((extension_api, min, max)),
        )
    }

    fn upgrade_note() -> String {
        format!(
            "sessions on generation {PROTOCOL_VERSION} need Upgrade session · ends their programs"
        )
    }

    /// A release whose window still holds this build's generation reaches every session running
    /// now, even when it speaks a newer one.
    #[test]
    fn a_release_that_keeps_this_generation_says_sessions_keep_working() {
        for (min, max) in [
            (PROTOCOL_VERSION, PROTOCOL_VERSION),
            (PROTOCOL_VERSION, PROTOCOL_VERSION + 1),
            (PROTOCOL_VERSION - 1, PROTOCOL_VERSION + 1),
        ] {
            let update = compatibility(EXTENSION_API_VERSION, min, max);
            assert!(!update.needs_caution(), "{min}..={max}");
            let running = env!("CARGO_PKG_VERSION");
            assert_eq!(
                update.toast_body(false),
                format!("v{running} → v9.0.0\nrun `rozi update`\nrunning sessions keep working")
            );
            assert!(
                update
                    .installed_body()
                    .ends_with("\nrunning sessions keep working")
            );
        }
    }

    /// Outside the window - newer or older - sessions running now cannot be reached until they
    /// are upgraded, so the notice is a warning and names the step.
    #[test]
    fn a_release_past_this_generation_says_sessions_need_upgrade_session() {
        for (min, max) in [
            (PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 2),
            (PROTOCOL_VERSION - 2, PROTOCOL_VERSION - 1),
        ] {
            let update = compatibility(EXTENSION_API_VERSION, min, max);
            assert!(update.needs_caution(), "{min}..={max}");
            let body = update.toast_body(false);
            assert_eq!(body.lines().nth(2), Some(upgrade_note().as_str()), "{body}");
            assert_eq!(body.lines().count(), 3, "{body}");
            assert!(update.installed_body().ends_with(&upgrade_note()));
        }
    }

    #[test]
    fn an_extension_api_step_gets_its_own_row_above_the_session_row() {
        let next = EXTENSION_API_VERSION + 1;
        let extension_row =
            format!("extension API {EXTENSION_API_VERSION}→{next} · review extensions first");

        let kept = compatibility(next, PROTOCOL_VERSION, PROTOCOL_VERSION);
        assert!(kept.needs_caution());
        let body = kept.toast_body(false);
        let rows: Vec<&str> = body.lines().skip(2).collect();
        assert_eq!(
            rows,
            [extension_row.as_str(), "running sessions keep working"]
        );

        let moved = compatibility(next, PROTOCOL_VERSION + 1, PROTOCOL_VERSION + 1);
        let body = moved.toast_body(false);
        let rows: Vec<&str> = body.lines().skip(2).collect();
        assert_eq!(rows, [extension_row.as_str(), upgrade_note().as_str()]);
    }

    /// Without release metadata the notice says nothing it cannot back up.
    #[test]
    fn unknown_compatibility_adds_no_rows() {
        let update = update(InstallSource::Managed);
        assert!(!update.needs_caution());
        assert_eq!(update.toast_body(false).lines().count(), 2);
        assert_eq!(
            update.installed_body(),
            "quit and start rozi again to use it"
        );
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

    fn fetched(document: &str, latest: &str) -> Option<ReleaseCompatibility> {
        let repository = Url::parse("https://github.com/tui-lipan/rozi/").unwrap();
        let downloader = FakeDownloader {
            bytes: document.as_bytes().to_vec(),
        };
        fetch_compatibility(&downloader, &repository, &Version::parse(latest).unwrap())
    }

    #[test]
    fn compatibility_document_must_match_the_signed_release_version() {
        let document = r#"{"schema_version":2,"version":"1.0.0","extension_api":2,"session_protocol_min":51,"session_protocol_max":52}"#;
        assert!(fetched(document, "2.0.0").is_none());

        let document = r#"{"schema_version":2,"version":"2.0.0","extension_api":2,"session_protocol_min":51,"session_protocol_max":52}"#;
        let compatibility = fetched(document, "2.0.0").unwrap();
        assert_eq!(compatibility.version, Version::new(2, 0, 0));
        assert_eq!(
            (
                compatibility.session_protocol_min,
                compatibility.session_protocol_max
            ),
            (51, 52)
        );
    }

    /// A document in another layout - the single-protocol schema 1, or one from a future release -
    /// is no information rather than a guess.
    #[test]
    fn a_compatibility_document_in_another_schema_is_ignored() {
        let schema_one =
            r#"{"schema_version":1,"version":"2.0.0","extension_api":2,"session_protocol":5}"#;
        assert!(fetched(schema_one, "2.0.0").is_none());
        let future = r#"{"schema_version":3,"version":"2.0.0","extension_api":2,"session_protocol_min":5,"session_protocol_max":5}"#;
        assert!(fetched(future, "2.0.0").is_none());
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
