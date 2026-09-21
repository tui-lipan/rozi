//! Bounded fetch and validation for the public extension discovery index.

use std::collections::HashSet;

use relswap::{Downloader, UreqDownloader};
use semver::Version;
use serde::Deserialize;
use url::Url;

use crate::config::EXTENSION_API_VERSION;

const INDEX_SCHEMA_VERSION: u32 = 1;
const INDEX_URL: &str = "https://tui-lipan.github.io/rozi-extension-index/v1/index.json";
const MAX_INDEX_BYTES: usize = 1024 * 1024;
const KNOWN_PLATFORMS: [&str; 5] = ["linux", "macos", "windows", "freebsd", "netbsd"];

#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
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
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CatalogDocument {
    schema_version: u32,
    generated_at: String,
    extensions: Vec<CatalogEntry>,
}

pub(crate) fn fetch() -> Result<Vec<CatalogEntry>, String> {
    fetch_with(&UreqDownloader::new())
}

fn fetch_with(downloader: &impl Downloader) -> Result<Vec<CatalogEntry>, String> {
    let url = Url::parse(INDEX_URL).expect("extension index URL is valid");
    let response = downloader
        .fetch(&url, MAX_INDEX_BYTES)
        .map_err(|error| format!("Could not fetch extension index: {error}"))?;
    let document: CatalogDocument = serde_json::from_slice(&response.bytes)
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
    for entry in &document.extensions {
        entry.validate()?;
    }
    Ok(document.extensions)
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
        fetch_with(&FakeDownloader {
            bytes: text.as_bytes().to_vec(),
        })
    }

    #[test]
    fn fetches_and_validates_the_v1_index() {
        let entries = parse(VALID).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "vim-rozi-navigator");
        assert_eq!(entries[0].incompatibility(), None);
    }

    #[test]
    fn rejects_unknown_schema_and_unpinned_sources() {
        let schema = VALID.replacen("\"schema_version\": 1", "\"schema_version\": 2", 1);
        assert!(parse(&schema).unwrap_err().contains("schema 2"));

        let moving = VALID.replacen("5b5c8b9323e260a7c10a63d792274ca155d51e26", "master", 1);
        assert!(parse(&moving).unwrap_err().contains("invalid commit"));
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
