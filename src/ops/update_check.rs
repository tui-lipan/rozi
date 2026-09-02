//! Quiet startup update checks and the compatibility warning derived from release metadata.

use relswap::{Downloader, UreqDownloader};
use semver::Version;
use serde::Deserialize;
use std::fs::OpenOptions;
use std::io::ErrorKind;
use std::path::Path;
use url::Url;

use crate::config::EXTENSION_API_VERSION;
use crate::platform::install_source::InstallSource;
use crate::release_app::ROZI;
use crate::session::protocol::PROTOCOL_VERSION;

const COMPATIBILITY_SCHEMA_VERSION: u32 = 1;
const COMPATIBILITY_FILE: &str = "rozi-compatibility.json";
const MAX_COMPATIBILITY_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StartupUpdate {
    pub(crate) latest: Version,
    pub(crate) hint: String,
    compatibility: Option<ReleaseCompatibility>,
}

impl StartupUpdate {
    pub(crate) fn compatibility_warning(&self) -> Option<String> {
        let compatibility = self.compatibility.as_ref()?;
        let extension_bump = (compatibility.extension_api > EXTENSION_API_VERSION)
            .then_some(compatibility.extension_api);
        let protocol_bump = (compatibility.session_protocol > PROTOCOL_VERSION)
            .then_some(compatibility.session_protocol);

        match (extension_bump, protocol_bump) {
            (Some(extension_api), Some(protocol)) => Some(format!(
                "Extension API {EXTENSION_API_VERSION} -> {extension_api}; session protocol \
                 {PROTOCOL_VERSION} -> {protocol}. Review extensions and restart sessions after \
                 updating."
            )),
            (Some(extension_api), None) => Some(format!(
                "Extension API {EXTENSION_API_VERSION} -> {extension_api}. Review extensions \
                 before updating."
            )),
            (None, Some(protocol)) => Some(format!(
                "Session protocol {PROTOCOL_VERSION} -> {protocol}. Restart running sessions after \
                 updating."
            )),
            (None, None) => None,
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

/// Check signed latest-release metadata without delaying startup.
///
/// The caller runs this on a worker thread. Network and compatibility-metadata failures stay
/// silent: an update toast is useful, but a machine being offline is not a startup error.
pub(crate) fn check_startup() -> Option<StartupUpdate> {
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
    if !claim_startup_notice(&latest) {
        return None;
    }
    Some(StartupUpdate {
        latest,
        hint: update_hint(crate::platform::install_source::detect_current()),
        compatibility,
    })
}

/// Atomically let one client announce each release. If state storage is unavailable, prefer a
/// repeated useful notice over hiding updates forever.
fn claim_startup_notice(latest: &Version) -> bool {
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

fn update_hint(source: InstallSource) -> String {
    match source {
        InstallSource::Managed => "Run `rozi update`.".to_string(),
        InstallSource::SystemPackage => "Update with your system package manager.".to_string(),
        InstallSource::Unknown => {
            format!("See {}/installation", env!("CARGO_PKG_HOMEPAGE"))
        }
        source => format!(
            "Run `{}`.",
            source
                .upgrade_command()
                .expect("package-manager sources have an upgrade command")
        ),
    }
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

    fn compatibility(version: &str, extension_api: u32, session_protocol: u32) -> StartupUpdate {
        StartupUpdate {
            latest: Version::parse(version).unwrap(),
            hint: "Run `rozi update`.".to_string(),
            compatibility: Some(ReleaseCompatibility {
                schema_version: COMPATIBILITY_SCHEMA_VERSION,
                version: Version::parse(version).unwrap(),
                extension_api,
                session_protocol,
            }),
        }
    }

    #[test]
    fn compatibility_warning_names_each_contract_that_moves_forward() {
        let both = compatibility("9.0.0", EXTENSION_API_VERSION + 1, PROTOCOL_VERSION + 1);
        let warning = both.compatibility_warning().unwrap();
        assert!(warning.contains("Extension API"));
        assert!(warning.contains("session protocol"));

        assert!(
            compatibility("9.0.0", EXTENSION_API_VERSION, PROTOCOL_VERSION)
                .compatibility_warning()
                .is_none()
        );
    }

    #[test]
    fn compatibility_document_must_match_the_signed_release_version() {
        let repository = Url::parse("https://github.com/tui-lipan/rozi/").unwrap();
        let latest = Version::parse("2.0.0").unwrap();
        let wrong_version = FakeDownloader {
            bytes:
                br#"{"schema_version":1,"version":"1.0.0","extension_api":2,"session_protocol":4}"#
                    .to_vec(),
        };
        assert!(fetch_compatibility(&wrong_version, &repository, &latest).is_none());

        let matching = FakeDownloader {
            bytes:
                br#"{"schema_version":1,"version":"2.0.0","extension_api":2,"session_protocol":4}"#
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
    fn update_hint_respects_the_install_owner() {
        assert_eq!(update_hint(InstallSource::Managed), "Run `rozi update`.");
        assert_eq!(
            update_hint(InstallSource::Cargo),
            "Run `cargo install rozi --locked`."
        );
        assert!(update_hint(InstallSource::SystemPackage).contains("system package manager"));
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
