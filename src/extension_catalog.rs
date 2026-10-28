//! Bounded fetch and validation for the public extension discovery index.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use relswap::{Downloader, UreqDownloader};
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::config::EXTENSION_API_VERSION;

const INDEX_SCHEMA_VERSION: u32 = 1;
const INDEX_URL: &str = "https://tui-lipan.github.io/rozi-extension-index/v1/index.json";
/// Keep equal to `MAX_INDEX_BYTES` in the index generator, which sheds entries to stay under it.
const MAX_INDEX_BYTES: usize = 1024 * 1024;
/// How long a cached index is shown without asking the network, matching the index host's
/// `Cache-Control: max-age`. The index is rebuilt hourly, so this only spares repeated opens.
const CACHE_FRESH_FOR: Duration = Duration::from_secs(10 * 60);
pub(crate) const KNOWN_PLATFORMS: [&str; 5] = ["linux", "macos", "windows", "freebsd", "netbsd"];
/// Longest accepted value, in characters, of each metadata field. Keep equal to `MAX_FIELD_CHARS`
/// in the index generator, so a record it publishes is one Rozi accepts.
pub(crate) const FIELD_LIMITS: [(&str, usize); 6] = [
    ("id", 64),
    ("title", 80),
    ("description", 280),
    ("version", 64),
    ("min_rozi", 64),
    ("homepage", 256),
];
/// Most screenshots one extension lists, and the largest file each may be. Keep equal to
/// `MAX_SCREENSHOTS` and `MAX_SCREENSHOT_BYTES` in the index generator.
pub(crate) const MAX_SCREENSHOTS: usize = 4;
pub(crate) const MAX_SCREENSHOT_BYTES: u64 = 2 * 1024 * 1024;
const MAX_SCREENSHOT_PATH_CHARS: usize = 256;
const SCREENSHOT_EXTENSIONS: [&str; 4] = ["png", "jpg", "jpeg", "webp"];

/// Why a manifest's screenshot path is not one the index lists, or `None` when it is.
///
/// The path names a file in the extension's repository and becomes part of a URL a web page
/// loads, so it is held to a plain repository-relative spelling with no escapes or traversal.
pub(crate) fn screenshot_path_problem(path: &str) -> Option<String> {
    if path.is_empty() || path.chars().count() > MAX_SCREENSHOT_PATH_CHARS {
        return Some(format!(
            "must be 1 to {MAX_SCREENSHOT_PATH_CHARS} characters"
        ));
    }
    if !path
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'/'))
    {
        return Some("may use only ASCII letters, digits, `.`, `_`, `-`, and `/`".to_string());
    }
    if path
        .split('/')
        .any(|segment| matches!(segment, "" | "." | ".."))
    {
        return Some(
            "must be relative to the repository root, without empty, `.`, or `..` segments"
                .to_string(),
        );
    }
    let extension = path.rsplit_once('.').map(|(_, extension)| extension);
    if !extension.is_some_and(|extension| SCREENSHOT_EXTENSIONS.contains(&extension)) {
        return Some(format!(
            "must end in .{}",
            SCREENSHOT_EXTENSIONS.join(", .")
        ));
    }
    None
}

/// Fields this Rozi does not know are ignored, here and in [`CatalogDocument`]: the index adds
/// fields within a schema version, and rejecting them would drop every record for older clients.
/// The generator, not the reader, is strict about what it publishes.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub(crate) repository: String,
    pub(crate) source: String,
    pub(crate) commit: String,
    pub(crate) manifest_path: String,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) api: u32,
    pub(crate) min_rozi: Option<String>,
    #[serde(default)]
    pub(crate) platforms: Vec<String>,
    pub(crate) homepage: Option<String>,
    /// Repository-relative image paths at `commit`, for the index website. Rozi does not show them.
    #[serde(default)]
    pub(crate) screenshots: Vec<String>,
    pub(crate) stars: u64,
    pub(crate) updated_at: String,
    pub(crate) commands: usize,
    pub(crate) services: usize,
    pub(crate) agents: usize,
    pub(crate) sidebar_tabs: usize,
    pub(crate) navigation_targets: usize,
    pub(crate) suggested_keybindings: usize,
}

impl CatalogEntry {
    /// Trust is assigned by rozi to its author's extensions, never by extension metadata.
    pub(crate) fn is_trusted(&self) -> bool {
        matches!(
            self.repository.as_str(),
            "tui-lipan/claude-rozi-sessions"
                | "tui-lipan/codex-rozi-sessions"
                | "tui-lipan/rozi-terminal-browser"
                | "tui-lipan/vim-rozi-navigator"
        )
    }

    /// The `owner/name` GitHub repository, which identifies an entry across index refreshes.
    pub fn repository(&self) -> &str {
        &self.repository
    }

    pub(crate) fn incompatibility(&self) -> Option<String> {
        if self.api != EXTENSION_API_VERSION {
            return Some(format!(
                "requires extension API {}, rozi supports API {EXTENSION_API_VERSION}",
                self.api
            ));
        }
        if !self.platforms.is_empty()
            && !self
                .platforms
                .iter()
                .any(|platform| platform == std::env::consts::OS)
        {
            return Some(format!(
                "supports {} but this is {}",
                self.platforms.join(", "),
                std::env::consts::OS
            ));
        }
        let minimum = self
            .min_rozi
            .as_deref()
            .and_then(|version| version.parse::<Version>().ok())?;
        let current =
            Version::parse(env!("CARGO_PKG_VERSION")).expect("rozi's own version is valid semver");
        (current < minimum).then(|| format!("needs rozi {minimum} or newer, this is {current}"))
    }

    fn validate(&self) -> Result<(), String> {
        for (name, value) in [
            ("repository", self.repository.as_str()),
            ("source", self.source.as_str()),
            ("id", self.id.as_str()),
            ("title", self.title.as_str()),
            ("description", self.description.as_str()),
            ("version", self.version.as_str()),
            ("updated_at", self.updated_at.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(format!("catalog entry has an empty `{name}`"));
            }
        }
        for ((name, limit), value) in FIELD_LIMITS.into_iter().zip([
            Some(self.id.as_str()),
            Some(self.title.as_str()),
            Some(self.description.as_str()),
            Some(self.version.as_str()),
            self.min_rozi.as_deref(),
            self.homepage.as_deref(),
        ]) {
            if value.is_some_and(|value| value.chars().count() > limit) {
                return Err(format!(
                    "catalog entry `{}` has a `{name}` longer than {limit} characters",
                    self.repository
                ));
            }
        }
        if self.manifest_path != "extension.toml" {
            return Err(format!(
                "catalog entry `{}` has unsupported manifest path `{}`",
                self.repository, self.manifest_path
            ));
        }
        if self.commit.len() != 40
            || !self
                .commit
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(format!(
                "catalog entry `{}` has an invalid commit",
                self.repository
            ));
        }
        let expected_source = format!("https://github.com/{}.git", self.repository);
        let source = Url::parse(&self.source)
            .map_err(|_| format!("catalog entry `{}` has an invalid source", self.repository))?;
        if self.source != expected_source
            || source.scheme() != "https"
            || source.host_str() != Some("github.com")
        {
            return Err(format!(
                "catalog entry `{}` has a non-canonical source",
                self.repository
            ));
        }
        if self.id.is_empty()
            || !self.id.bytes().all(|byte| {
                byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_' || byte == b'-'
            })
        {
            return Err(format!(
                "catalog entry `{}` has an invalid extension id",
                self.repository
            ));
        }
        let mut seen = HashSet::new();
        if self
            .platforms
            .iter()
            .any(|platform| !KNOWN_PLATFORMS.contains(&platform.as_str()) || !seen.insert(platform))
        {
            return Err(format!(
                "catalog entry `{}` has invalid platforms",
                self.repository
            ));
        }
        if let Some(minimum) = self.min_rozi.as_deref()
            && minimum.parse::<Version>().is_err()
        {
            return Err(format!(
                "catalog entry `{}` has invalid min_rozi",
                self.repository
            ));
        }
        if let Some(homepage) = self.homepage.as_deref() {
            let valid = Url::parse(homepage).is_ok_and(|url| {
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some()
            });
            if !valid {
                return Err(format!(
                    "catalog entry `{}` has an invalid homepage",
                    self.repository
                ));
            }
        }
        let mut seen = HashSet::new();
        if self.screenshots.len() > MAX_SCREENSHOTS
            || self
                .screenshots
                .iter()
                .any(|path| screenshot_path_problem(path).is_some() || !seen.insert(path.as_str()))
        {
            return Err(format!(
                "catalog entry `{}` has invalid screenshots",
                self.repository
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct CatalogDocument {
    schema_version: u32,
    generated_at: String,
    /// Kept raw so each record is parsed on its own: one bad record must not hide the rest.
    extensions: Vec<serde_json::Value>,
}

/// The last index Rozi fetched, read back from the cache directory.
pub(crate) struct CachedCatalog {
    pub(crate) entries: Vec<CatalogEntry>,
    /// Young enough to show without refreshing.
    pub(crate) fresh: bool,
}

/// Fetches the index and, on success, keeps its bytes for [`cached`].
pub(crate) fn fetch() -> Result<Vec<CatalogEntry>, String> {
    let bytes = download(&UreqDownloader::new())?;
    let entries = parse(&bytes)?;
    // A cache that cannot be written only costs the next open its head start.
    let _ = store(&cache_path(), &bytes);
    Ok(entries)
}

/// The cached index, so the manager can list it before any network round trip. The bytes are
/// validated exactly like a fresh download, since the cache is only a copy of one.
pub(crate) fn cached() -> Option<CachedCatalog> {
    load(&cache_path(), SystemTime::now())
}

fn cache_path() -> PathBuf {
    crate::platform::paths::cache_dir(&crate::platform::paths::PlatformEnv::from_process())
        .join("extension-index-v1.json")
}

fn store(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    crate::platform::persist::replace_file(path, bytes)
}

fn load(path: &Path, now: SystemTime) -> Option<CachedCatalog> {
    let metadata = std::fs::metadata(path).ok()?;
    if metadata.len() > MAX_INDEX_BYTES as u64 {
        return None;
    }
    let entries = parse(&std::fs::read(path).ok()?).ok()?;
    // A modification time in the future is a clock change, not freshness.
    let fresh = metadata
        .modified()
        .ok()
        .and_then(|modified| now.duration_since(modified).ok())
        .is_some_and(|age| age < CACHE_FRESH_FOR);
    Some(CachedCatalog { entries, fresh })
}

fn download(downloader: &impl Downloader) -> Result<Vec<u8>, String> {
    let url = Url::parse(INDEX_URL).expect("extension index URL is valid");
    downloader
        .fetch(&url, MAX_INDEX_BYTES)
        .map(|response| response.bytes)
        .map_err(|error| format!("Could not fetch extension index: {error}"))
}

fn parse(bytes: &[u8]) -> Result<Vec<CatalogEntry>, String> {
    let document: CatalogDocument = serde_json::from_slice(bytes)
        .map_err(|error| format!("Could not read extension index: {error}"))?;
    if document.schema_version != INDEX_SCHEMA_VERSION {
        return Err(format!(
            "Extension index uses schema {}, rozi supports schema {INDEX_SCHEMA_VERSION}",
            document.schema_version
        ));
    }
    if document.generated_at.trim().is_empty() {
        return Err("Extension index has no generation time".to_string());
    }
    // The document is Rozi's to get right, so a malformed one fails discovery above. Each record
    // describes a third-party repository anyone can opt in, so one that is malformed, invalid, or
    // a repeat of an earlier repository is dropped on its own.
    let mut repositories = HashSet::new();
    Ok(document
        .extensions
        .into_iter()
        .filter_map(|value| serde_json::from_value::<CatalogEntry>(value).ok())
        .filter(|entry| entry.validate().is_ok() && repositories.insert(entry.repository.clone()))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use relswap::{DownloadResponse, ReleaseError};

    const VALID: &str = r#"{
      "schema_version": 1,
      "generated_at": "2026-09-21T00:00:00Z",
      "extensions": [{
        "repository": "tui-lipan/vim-rozi-navigator",
        "source": "https://github.com/tui-lipan/vim-rozi-navigator.git",
        "commit": "5b5c8b9323e260a7c10a63d792274ca155d51e26",
        "manifest_path": "extension.toml",
        "id": "vim-rozi-navigator",
        "title": "Vim and Neovim navigator",
        "description": "Split-aware navigation policy",
        "version": "0.2.1",
        "api": 1,
        "min_rozi": "0.0.16",
        "platforms": [],
        "homepage": "https://github.com/tui-lipan/vim-rozi-navigator",
        "stars": 0,
        "updated_at": "2026-09-21T00:00:00Z",
        "commands": 0,
        "services": 0,
        "agents": 0,
        "sidebar_tabs": 0,
        "navigation_targets": 1,
        "suggested_keybindings": 4
      }]
    }"#;

    struct FakeDownloader {
        bytes: Vec<u8>,
    }

    impl Downloader for FakeDownloader {
        fn fetch(&self, url: &Url, max_bytes: usize) -> Result<DownloadResponse, ReleaseError> {
            assert_eq!(url.as_str(), INDEX_URL);
            assert_eq!(max_bytes, MAX_INDEX_BYTES);
            Ok(DownloadResponse::new(
                url.clone(),
                url.clone(),
                Vec::new(),
                self.bytes.clone(),
            ))
        }
    }

    fn parse(text: &str) -> Result<Vec<CatalogEntry>, String> {
        let bytes = download(&FakeDownloader {
            bytes: text.as_bytes().to_vec(),
        })?;
        super::parse(&bytes)
    }

    #[test]
    fn fetches_and_validates_the_v1_index() {
        let entries = parse(VALID).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "vim-rozi-navigator");
        assert_eq!(entries[0].incompatibility(), None);
    }

    #[test]
    fn rejects_unknown_schema_and_skips_unpinned_sources() {
        let schema = VALID.replacen("\"schema_version\": 1", "\"schema_version\": 2", 1);
        assert!(parse(&schema).unwrap_err().contains("schema 2"));

        let moving = VALID.replacen("5b5c8b9323e260a7c10a63d792274ca155d51e26", "master", 1);
        assert!(parse(&moving).unwrap().is_empty());
    }

    /// Anyone can opt a repository in with a topic, so one bad record must not take discovery
    /// offline for every client.
    #[test]
    fn one_bad_record_does_not_hide_the_rest() {
        let document: serde_json::Value = serde_json::from_str(VALID).unwrap();
        let good = document["extensions"][0].clone();
        let mut empty_host = good.clone();
        empty_host["repository"] = "someone/bad-homepage".into();
        empty_host["source"] = "https://github.com/someone/bad-homepage.git".into();
        empty_host["homepage"] = "https://".into();
        let mut mistyped = good.clone();
        mistyped["repository"] = "someone/mistyped".into();
        mistyped["stars"] = "many".into();
        let text = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-09-21T00:00:00Z",
            "extensions": [empty_host, mistyped, good.clone(), good],
        })
        .to_string();

        let entries = parse(&text).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].repository, "tui-lipan/vim-rozi-navigator");
    }

    /// The index adds fields within a schema version; a client that predates one keeps listing
    /// every record.
    #[test]
    fn fields_a_later_index_adds_are_ignored() {
        let mut document: serde_json::Value = serde_json::from_str(VALID).unwrap();
        document["published_by"] = "a later generator".into();
        document["extensions"][0]["surprise"] = true.into();

        let entries = parse(&document.to_string()).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(CatalogEntry::repository)
                .collect::<Vec<_>>(),
            ["tui-lipan/vim-rozi-navigator"]
        );
    }

    #[test]
    fn skips_records_over_the_generator_field_limits() {
        let document: serde_json::Value = serde_json::from_str(VALID).unwrap();
        let good = document["extensions"][0].clone();
        let mut at_limit = good.clone();
        at_limit["repository"] = "someone/at-limit".into();
        at_limit["source"] = "https://github.com/someone/at-limit.git".into();
        at_limit["description"] = "é".repeat(280).into();
        let mut over_limit = good.clone();
        over_limit["description"] = "d".repeat(281).into();
        let text = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-09-21T00:00:00Z",
            "extensions": [over_limit, at_limit],
        })
        .to_string();

        let entries = parse(&text).unwrap();
        assert_eq!(entries.len(), 1, "limits count characters, not bytes");
        assert_eq!(entries[0].repository, "someone/at-limit");
    }

    #[test]
    fn screenshots_are_optional_but_must_be_plain_repository_paths() {
        let document: serde_json::Value = serde_json::from_str(VALID).unwrap();
        let entry = |name: &str, screenshots: serde_json::Value| {
            let mut entry = document["extensions"][0].clone();
            entry["repository"] = format!("someone/{name}").into();
            entry["source"] = format!("https://github.com/someone/{name}.git").into();
            entry["screenshots"] = screenshots;
            entry
        };
        let text = serde_json::json!({
            "schema_version": 1,
            "generated_at": "2026-09-21T00:00:00Z",
            "extensions": [
                entry("listed", serde_json::json!(["docs/picker.png", "docs/sidebar.webp"])),
                entry("traversal", serde_json::json!(["../secret.png"])),
                entry("not-an-image", serde_json::json!(["docs/run.sh"])),
                entry("repeated", serde_json::json!(["a.png", "a.png"])),
                entry("too-many", serde_json::json!(["1.png", "2.png", "3.png", "4.png", "5.png"])),
            ],
        })
        .to_string();

        let entries = parse(&text).unwrap();
        assert_eq!(
            entries
                .iter()
                .map(CatalogEntry::repository)
                .collect::<Vec<_>>(),
            ["someone/listed"]
        );
        assert_eq!(
            entries[0].screenshots,
            ["docs/picker.png", "docs/sidebar.webp"]
        );
        assert_eq!(parse(VALID).unwrap()[0].screenshots, Vec::<String>::new());
    }

    #[test]
    fn minimum_versions_match_the_discover_conformance_corpus() {
        let cases: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/discover-eligibility.json"))
                .unwrap();
        for case in cases.as_array().unwrap().iter().filter(|case| {
            case["name"]
                .as_str()
                .unwrap()
                .starts_with("minimum version ")
        }) {
            let manifest: toml::Value = toml::from_str(case["manifest"].as_str().unwrap()).unwrap();
            let minimum = manifest["extension"]["min_rozi"].as_str().unwrap();
            let document = VALID.replace("0.0.16", minimum);
            assert_eq!(
                !parse(&document).unwrap().is_empty(),
                case["eligible"].as_bool().unwrap(),
                "{minimum}"
            );
        }
    }

    #[test]
    fn cached_index_is_revalidated_and_ages_out() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cache/extension-index-v1.json");
        assert!(load(&path, SystemTime::now()).is_none());

        store(&path, VALID.as_bytes()).unwrap();
        let written = std::fs::metadata(&path).unwrap().modified().unwrap();
        let cached = load(&path, written + Duration::from_secs(60)).unwrap();
        assert_eq!(cached.entries.len(), 1);
        assert!(cached.fresh);
        assert!(!load(&path, written + CACHE_FRESH_FOR).unwrap().fresh);
        assert!(
            !load(&path, written - Duration::from_secs(60))
                .unwrap()
                .fresh,
            "a clock that moved backwards does not make the cache fresh"
        );

        std::fs::write(&path, b"{ not json").unwrap();
        assert!(load(&path, written).is_none(), "a corrupt cache is ignored");
    }

    #[test]
    fn a_malformed_document_still_fails_discovery() {
        assert!(parse("{\"schema_version\": 1}").is_err());
        let no_list = VALID.replacen("\"extensions\": [", "\"extension\": [", 1);
        assert!(parse(&no_list).is_err());
    }

    #[test]
    fn reports_api_platform_and_version_incompatibility() {
        let mut entry = parse(VALID).unwrap().remove(0);
        entry.api = 2;
        assert!(entry.incompatibility().unwrap().contains("API 2"));
        entry.api = 1;
        entry.platforms = vec![if cfg!(target_os = "windows") {
            "linux".to_string()
        } else {
            "windows".to_string()
        }];
        assert!(entry.incompatibility().unwrap().contains("supports"));
        entry.platforms.clear();
        entry.min_rozi = Some("999.0.0".to_string());
        assert!(entry.incompatibility().unwrap().contains("needs rozi"));
    }
}
