//! HTTPS release metadata resolution and bounded archive downloads.

use super::archive::verify_archive_bytes;
use super::manifest::ReleaseManifest;
use super::signature::{
    SignatureEnvelope, TrustedKey, VerifiedSignature, verify_manifest, verify_manifest_with_keys,
};
use super::target::Target;
use super::{MAX_ARCHIVE_SIZE, MAX_METADATA_SIZE, ReleaseError, Result};
use std::collections::BTreeSet;
use std::time::Duration;
use ureq::ResponseExt;
use url::Url;

pub const METADATA_FILENAME: &str = "hyprmux-release.json";
pub const SIGNATURE_FILENAME: &str = "hyprmux-release.signatures.json";
pub const MAX_REDIRECTS: u32 = 8;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The result returned by an injected or production downloader.
#[derive(Clone, Debug)]
pub struct DownloadResponse {
    pub requested_url: Url,
    pub final_url: Url,
    /// Includes whatever redirect history the transport observed. The requested and final URLs
    /// are also considered by the release resolver even when a test seam supplies an empty list.
    pub redirect_history: Vec<Url>,
    pub bytes: Vec<u8>,
}

impl DownloadResponse {
    pub fn new(
        requested_url: Url,
        final_url: Url,
        redirect_history: Vec<Url>,
        bytes: Vec<u8>,
    ) -> Self {
        Self {
            requested_url,
            final_url,
            redirect_history,
            bytes,
        }
    }
}

/// Injectable network boundary used by metadata and archive operations.
pub trait Downloader {
    fn fetch(&self, url: &Url, max_bytes: usize) -> Result<DownloadResponse>;
}

/// A configured ureq/rustls downloader for production operations.
#[derive(Clone)]
pub struct UreqDownloader {
    agent: ureq::Agent,
}

impl UreqDownloader {
    pub fn new() -> Self {
        let config = ureq::Agent::config_builder()
            .https_only(true)
            .max_redirects(MAX_REDIRECTS)
            .max_redirects_will_error(true)
            .save_redirect_history(true)
            .timeout_global(Some(REQUEST_TIMEOUT))
            .timeout_connect(Some(REQUEST_TIMEOUT))
            .timeout_recv_response(Some(REQUEST_TIMEOUT))
            .timeout_recv_body(Some(REQUEST_TIMEOUT))
            .build();
        Self {
            agent: ureq::Agent::new_with_config(config),
        }
    }
}

impl Default for UreqDownloader {
    fn default() -> Self {
        Self::new()
    }
}

impl Downloader for UreqDownloader {
    fn fetch(&self, url: &Url, max_bytes: usize) -> Result<DownloadResponse> {
        require_https(url)?;
        let mut response = self
            .agent
            .get(url.as_str())
            .call()
            .map_err(|error| ReleaseError::download(error.to_string()))?;
        let final_url = Url::parse(response.get_uri().to_string().as_str()).map_err(|error| {
            ReleaseError::download(format!("invalid final response URL: {error}"))
        })?;
        require_https(&final_url)?;
        let redirect_history = response
            .get_redirect_history()
            .unwrap_or(&[])
            .iter()
            .map(|uri| {
                Url::parse(uri.to_string().as_str()).map_err(|error| {
                    ReleaseError::download(format!("invalid redirect URL: {error}"))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let bytes = read_response_body(max_bytes, |limit| {
            response.body_mut().with_config().limit(limit).read_to_vec()
        })?;
        Ok(DownloadResponse::new(
            url.clone(),
            final_url,
            redirect_history,
            bytes,
        ))
    }
}

fn read_response_body<F, E>(max_bytes: usize, read: F) -> Result<Vec<u8>>
where
    F: FnOnce(u64) -> std::result::Result<Vec<u8>, E>,
    E: std::fmt::Display,
{
    let read_limit = max_bytes
        .checked_add(1)
        .ok_or_else(|| ReleaseError::download("download size limit overflow"))?;
    let read_limit = u64::try_from(read_limit)
        .map_err(|_| ReleaseError::download("download size limit exceeds u64"))?;
    let bytes = read(read_limit).map_err(|error| ReleaseError::download(error.to_string()))?;
    if bytes.len() > max_bytes {
        return Err(ReleaseError::download(format!(
            "response body exceeds maximum size {max_bytes}"
        )));
    }
    Ok(bytes)
}

/// Metadata whose manifest bytes have been signature-checked and parsed.
#[derive(Clone, Debug)]
pub struct ReleaseMetadata {
    pub version: semver::Version,
    pub manifest_bytes: Vec<u8>,
    pub manifest: ReleaseManifest,
    pub signature_bytes: Vec<u8>,
    pub signature: SignatureEnvelope,
    pub verified_signature: VerifiedSignature,
    /// Exact version-specific GitHub release download base, including its trailing slash.
    pub release_base: Url,
}

/// An archive whose bytes have been checked against the selected manifest asset.
#[derive(Clone, Debug)]
pub struct DownloadedArchive {
    pub target: Target,
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Fetch and verify the latest release metadata. This never downloads an archive.
pub fn fetch_latest_metadata<D: Downloader>(
    downloader: &D,
    repository: &Url,
) -> Result<ReleaseMetadata> {
    let response = downloader.fetch(&latest_metadata_url(repository)?, MAX_METADATA_SIZE)?;
    resolve_latest_response(downloader, repository, response, None)
}

/// Inject trusted keys while resolving latest metadata. This is the deterministic test/tooling seam.
pub fn fetch_latest_metadata_with_keys<D: Downloader>(
    downloader: &D,
    repository: &Url,
    trusted_keys: &[TrustedKey],
) -> Result<ReleaseMetadata> {
    let response = downloader.fetch(&latest_metadata_url(repository)?, MAX_METADATA_SIZE)?;
    resolve_latest_response(downloader, repository, response, Some(trusted_keys))
}

/// Fetch and verify a specific release version without consulting the moving `latest` endpoint.
pub fn fetch_exact_metadata<D: Downloader>(
    downloader: &D,
    repository: &Url,
    version: &semver::Version,
) -> Result<ReleaseMetadata> {
    let response =
        downloader.fetch(&exact_metadata_url(repository, version)?, MAX_METADATA_SIZE)?;
    fetch_version_metadata_response(downloader, repository, response, version, None)
}

pub fn fetch_version_metadata_with_keys<D: Downloader>(
    downloader: &D,
    repository: &Url,
    version: &semver::Version,
    trusted_keys: &[TrustedKey],
) -> Result<ReleaseMetadata> {
    let response =
        downloader.fetch(&exact_metadata_url(repository, version)?, MAX_METADATA_SIZE)?;
    fetch_version_metadata_response(
        downloader,
        repository,
        response,
        version,
        Some(trusted_keys),
    )
}

/// Download and verify one selected target archive after metadata resolution.
pub fn download_archive<D: Downloader>(
    downloader: &D,
    metadata: &ReleaseMetadata,
    target: Target,
) -> Result<DownloadedArchive> {
    let selected = metadata.manifest.asset_for(target)?;
    let asset = selected.asset;
    let url = metadata
        .release_base
        .join(&asset.archive)
        .map_err(|error| ReleaseError::download(format!("invalid archive URL: {error}")))?;
    let response = downloader.fetch(&url, MAX_ARCHIVE_SIZE as usize)?;
    validate_response_transport(&response)?;
    reject_cross_release_redirects(&response, &metadata.version)?;
    verify_archive_bytes(&response.bytes, asset)?;
    Ok(DownloadedArchive {
        target,
        name: asset.archive.clone(),
        bytes: response.bytes,
    })
}

pub fn latest_metadata_url(repository: &Url) -> Result<Url> {
    repository_base(repository)?
        .join(&format!("releases/latest/download/{METADATA_FILENAME}"))
        .map_err(|error| ReleaseError::download(format!("invalid latest metadata URL: {error}")))
}

pub fn exact_metadata_url(repository: &Url, version: &semver::Version) -> Result<Url> {
    let base = repository_base(repository)?;
    base.join(format!("releases/download/v{version}/{METADATA_FILENAME}").as_str())
        .map_err(|error| ReleaseError::download(format!("invalid release metadata URL: {error}")))
}

fn resolve_latest_response<D: Downloader>(
    downloader: &D,
    _repository: &Url,
    response: DownloadResponse,
    trusted_keys: Option<&[TrustedKey]>,
) -> Result<ReleaseMetadata> {
    validate_response_transport(&response)?;
    let candidates = versioned_manifest_candidates(&response)?;
    let mut versions = BTreeSet::new();
    for (version, _) in &candidates {
        versions.insert(version.to_string());
    }
    if versions.len() != 1 {
        return Err(ReleaseError::download(
            "latest metadata redirect history names multiple release versions",
        ));
    }
    let (version, release_url) = candidates.into_iter().next().ok_or_else(|| {
        ReleaseError::download("latest metadata did not resolve to a versioned release URL")
    })?;
    let release_base = release_base_from_manifest_url(&release_url)?;
    fetch_verified_metadata(
        downloader,
        response.bytes,
        release_base,
        version,
        trusted_keys,
    )
}

fn fetch_version_metadata_response<D: Downloader>(
    downloader: &D,
    repository: &Url,
    response: DownloadResponse,
    version: &semver::Version,
    trusted_keys: Option<&[TrustedKey]>,
) -> Result<ReleaseMetadata> {
    validate_response_transport(&response)?;
    reject_cross_release_redirects(&response, version)?;
    let release_base = repository_base(repository)?
        .join(format!("releases/download/v{version}/").as_str())
        .map_err(|error| ReleaseError::download(format!("invalid release base URL: {error}")))?;
    fetch_verified_metadata(
        downloader,
        response.bytes,
        release_base,
        version.clone(),
        trusted_keys,
    )
}

fn fetch_verified_metadata<D: Downloader>(
    downloader: &D,
    manifest_bytes: Vec<u8>,
    release_base: Url,
    version: semver::Version,
    trusted_keys: Option<&[TrustedKey]>,
) -> Result<ReleaseMetadata> {
    if manifest_bytes.len() > MAX_METADATA_SIZE {
        return Err(ReleaseError::download(
            "release manifest exceeds metadata limit",
        ));
    }
    let signature_url = release_base
        .join(SIGNATURE_FILENAME)
        .map_err(|error| ReleaseError::download(format!("invalid signature URL: {error}")))?;
    let signature_response = downloader.fetch(&signature_url, MAX_METADATA_SIZE)?;
    validate_response_transport(&signature_response)?;
    reject_cross_release_redirects(&signature_response, &version)?;
    let verified_signature = match trusted_keys {
        Some(keys) => verify_manifest_with_keys(&manifest_bytes, &signature_response.bytes, keys)?,
        None => verify_manifest(&manifest_bytes, &signature_response.bytes)?,
    };
    let signature = SignatureEnvelope::from_bytes(&signature_response.bytes)?;
    let manifest = ReleaseManifest::from_bytes(&manifest_bytes)?;
    if manifest.version != version {
        return Err(ReleaseError::download(format!(
            "release manifest version {} differs from resolved version {version}",
            manifest.version
        )));
    }
    Ok(ReleaseMetadata {
        version,
        manifest_bytes,
        manifest,
        signature_bytes: signature_response.bytes,
        signature,
        verified_signature,
        release_base,
    })
}

fn repository_base(repository: &Url) -> Result<Url> {
    require_https(repository)?;
    if repository.query().is_some() || repository.fragment().is_some() {
        return Err(ReleaseError::download(
            "repository URL must not contain a query or fragment",
        ));
    }
    let mut base = repository.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    Ok(base)
}

fn release_base_from_manifest_url(url: &Url) -> Result<Url> {
    let (version, _) = versioned_manifest_url(url)?
        .ok_or_else(|| ReleaseError::download("URL is not a version-specific release manifest"))?;
    let suffix = format!("/releases/download/v{version}/{METADATA_FILENAME}");
    let path = url.path();
    let base_path = path
        .strip_suffix(&suffix)
        .map(|prefix| format!("{prefix}/releases/download/v{version}/"))
        .ok_or_else(|| ReleaseError::download("cannot derive exact release base URL"))?;
    let mut base = url.clone();
    base.set_query(None);
    base.set_fragment(None);
    base.set_path(&base_path);
    Ok(base)
}

fn validate_response_transport(response: &DownloadResponse) -> Result<()> {
    require_https(&response.requested_url)?;
    require_https(&response.final_url)?;
    for url in &response.redirect_history {
        require_https(url)?;
    }
    Ok(())
}

fn reject_cross_release_redirects(
    response: &DownloadResponse,
    expected: &semver::Version,
) -> Result<()> {
    for url in response_history(response) {
        if let Some((version, _)) = versioned_manifest_url(url)? {
            if version != *expected {
                return Err(ReleaseError::download(format!(
                    "redirect history points at release {version}, expected {expected}"
                )));
            }
        } else if let Some(version) = versioned_release_version(url)?
            && version != *expected
        {
            return Err(ReleaseError::download(format!(
                "redirect history points at release {version}, expected {expected}"
            )));
        }
    }
    Ok(())
}

fn versioned_manifest_candidates(
    response: &DownloadResponse,
) -> Result<Vec<(semver::Version, Url)>> {
    let mut output = Vec::new();
    for url in response_history(response) {
        if let Some((version, _)) = versioned_manifest_url(url)? {
            output.push((version, url.clone()));
        }
    }
    Ok(output)
}

fn response_history(response: &DownloadResponse) -> Vec<&Url> {
    let mut output = Vec::with_capacity(response.redirect_history.len() + 2);
    output.extend(response.redirect_history.iter());
    output.push(&response.requested_url);
    output.push(&response.final_url);
    output
}

fn versioned_manifest_url(url: &Url) -> Result<Option<(semver::Version, Url)>> {
    let segments = url
        .path_segments()
        .ok_or_else(|| ReleaseError::download("release URL has no path segments"))?
        .collect::<Vec<_>>();
    for index in 0..segments.len().saturating_sub(3) {
        if segments[index] != "releases"
            || segments[index + 1] != "download"
            || segments[index + 3] != METADATA_FILENAME
            || index + 4 != segments.len()
        {
            continue;
        }
        let tag = segments[index + 2];
        let raw = tag
            .strip_prefix('v')
            .ok_or_else(|| ReleaseError::download("versioned release URL is missing v prefix"))?;
        let version = semver::Version::parse(raw).map_err(|error| {
            ReleaseError::download(format!("invalid release URL version: {error}"))
        })?;
        if version.to_string() != raw || url.query().is_some() || url.fragment().is_some() {
            return Err(ReleaseError::download(
                "release manifest redirect URL is not canonical",
            ));
        }
        return Ok(Some((version, url.clone())));
    }
    Ok(None)
}

fn versioned_release_version(url: &Url) -> Result<Option<semver::Version>> {
    let Some(segments) = url.path_segments() else {
        return Ok(None);
    };
    let segments = segments.collect::<Vec<_>>();
    for index in 0..segments.len().saturating_sub(2) {
        if segments[index] == "releases" && segments[index + 1] == "download" {
            let Some(tag) = segments[index + 2].strip_prefix('v') else {
                return Err(ReleaseError::download(
                    "versioned release redirect is missing v prefix",
                ));
            };
            let version = semver::Version::parse(tag).map_err(|error| {
                ReleaseError::download(format!("invalid release redirect version: {error}"))
            })?;
            if version.to_string() != tag {
                return Err(ReleaseError::download(
                    "release redirect version is not canonical",
                ));
            }
            return Ok(Some(version));
        }
    }
    Ok(None)
}

fn require_https(url: &Url) -> Result<()> {
    if url.scheme() != "https" {
        return Err(ReleaseError::download(format!(
            "release downloads require HTTPS, got {}",
            url.scheme()
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::signature::{TrustedKey, sign_manifest_bytes};
    use std::collections::{BTreeMap, HashMap};
    use std::sync::Mutex;

    struct MockDownloader {
        responses: Mutex<HashMap<String, DownloadResponse>>,
    }

    impl MockDownloader {
        fn new(responses: HashMap<String, DownloadResponse>) -> Self {
            Self {
                responses: Mutex::new(responses),
            }
        }
    }

    impl Downloader for MockDownloader {
        fn fetch(&self, url: &Url, max_bytes: usize) -> Result<DownloadResponse> {
            let response = self
                .responses
                .lock()
                .unwrap()
                .get(url.as_str())
                .cloned()
                .ok_or_else(|| ReleaseError::download(format!("unexpected URL {url}")))?;
            if response.bytes.len() > max_bytes {
                return Err(ReleaseError::download(
                    "mock response exceeded requested limit",
                ));
            }
            Ok(response)
        }
    }

    fn fixture() -> (Url, semver::Version, Vec<u8>, Vec<u8>, TrustedKey, String) {
        let repository = Url::parse("https://example.test/hyprmux/").unwrap();
        let version = semver::Version::parse("1.2.3").unwrap();
        let archive = b"archive bytes".to_vec();
        let asset = crate::release::manifest::ReleaseAsset::new(
            &version,
            Target::X86_64UnknownLinuxGnu,
            archive.len() as u64,
            crate::release::sha256_bytes(&archive),
            3,
            crate::release::sha256_bytes(b"bin"),
        );
        let target = Target::X86_64UnknownLinuxGnu;
        let manifest = ReleaseManifest::new(
            version.clone(),
            "2026-08-02T12:00:00Z",
            BTreeMap::from([(target, asset.clone())]),
        )
        .unwrap();
        let manifest_bytes = manifest.to_bytes().unwrap();
        let signing = ed25519_dalek::SigningKey::from_bytes(&[42; 32]);
        let trusted = TrustedKey::ed25519("stable", signing.verifying_key().to_bytes());
        let signature = sign_manifest_bytes(&manifest_bytes, "stable", &signing).unwrap();
        (
            repository,
            version,
            manifest_bytes,
            signature,
            trusted,
            asset.archive,
        )
    }

    fn response(
        requested: &str,
        final_url: &str,
        history: &[&str],
        bytes: Vec<u8>,
    ) -> DownloadResponse {
        DownloadResponse::new(
            Url::parse(requested).unwrap(),
            Url::parse(final_url).unwrap(),
            history.iter().map(|url| Url::parse(url).unwrap()).collect(),
            bytes,
        )
    }

    #[test]
    fn response_body_limit_accepts_exact_size_and_rejects_one_extra() {
        let exact = read_response_body(4, |limit| {
            assert_eq!(limit, 5);
            Ok::<_, &'static str>(vec![0; 4])
        })
        .unwrap();
        assert_eq!(exact.len(), 4);

        let too_large = read_response_body(4, |limit| {
            assert_eq!(limit, 5);
            Ok::<_, &'static str>(vec![0; 5])
        });
        assert!(
            matches!(too_large, Err(ReleaseError::Download(message)) if message.contains("exceeds maximum size 4"))
        );
    }

    #[test]
    fn latest_resolution_uses_exact_redirected_release_base() {
        let (repository, version, manifest, signature, trusted, archive_name) = fixture();
        let latest = latest_metadata_url(&repository).unwrap();
        let exact = format!(
            "https://example.test/hyprmux/releases/download/v{version}/{METADATA_FILENAME}"
        );
        let signature_url = format!(
            "https://example.test/hyprmux/releases/download/v{version}/{SIGNATURE_FILENAME}"
        );
        let archive_url =
            format!("https://example.test/hyprmux/releases/download/v{version}/{archive_name}");
        let archive = b"archive bytes".to_vec();
        let mut responses = HashMap::new();
        responses.insert(
            latest.to_string(),
            response(
                latest.as_str(),
                &exact,
                &[latest.as_str(), &exact],
                manifest,
            ),
        );
        responses.insert(
            signature_url.clone(),
            response(&signature_url, &signature_url, &[&signature_url], signature),
        );
        responses.insert(
            archive_url.clone(),
            response(&archive_url, &archive_url, &[&archive_url], archive),
        );
        let downloader = MockDownloader::new(responses);
        let metadata =
            fetch_latest_metadata_with_keys(&downloader, &repository, &[trusted]).unwrap();
        assert_eq!(metadata.version, version);
        let downloaded = download_archive(&downloader, &metadata, Target::X86_64UnknownLinuxGnu);
        assert!(downloaded.is_ok());
    }

    #[test]
    fn latest_cross_release_redirect_is_rejected() {
        let (repository, version, manifest, _signature, _trusted, _archive_name) = fixture();
        let latest = latest_metadata_url(&repository).unwrap();
        let other =
            format!("https://example.test/hyprmux/releases/download/v2.0.0/{METADATA_FILENAME}");
        let exact = format!(
            "https://example.test/hyprmux/releases/download/v{version}/{METADATA_FILENAME}"
        );
        let mut responses = HashMap::new();
        responses.insert(
            latest.to_string(),
            response(
                latest.as_str(),
                &exact,
                &[latest.as_str(), &other, &exact],
                manifest,
            ),
        );
        let downloader = MockDownloader::new(responses);
        assert!(fetch_latest_metadata_with_keys(&downloader, &repository, &[]).is_err());
    }
}
