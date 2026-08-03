#![cfg(feature = "release-tool")]

//! Maintainer-only release metadata tool.
//!
//! The wire formats in this file are deliberately small and boring.  The application owns the
//! corresponding `hyprmux::release::{target,manifest,signature,archive,hash}` modules; keeping the
//! tool at this boundary means a release job can be built once and used without a second compiler
//! invocation after a private key has been made available.

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::str::FromStr;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chrono::{DateTime, SecondsFormat, Utc};
use ed25519_dalek::SigningKey;
use flate2::read::GzDecoder;
use sha2::{Digest, Sha256};
use tar::Archive as TarArchive;
use zip::ZipArchive;

use hyprmux::release::{
    archive as release_archive,
    manifest::{ReleaseAsset, ReleaseManifest},
    sha256_file,
    signature::{self, SignatureEnvelope, TrustedKey, TrustedKeySet},
    target::Target,
};

const PUBLIC_KEY_ALGORITHM: &str = "ed25519";
const MANIFEST_NAME: &str = "hyprmux-release.json";
const SIGNATURES_NAME: &str = "hyprmux-release.signatures.json";

type Result<T> = std::result::Result<T, String>;

fn main() {
    if let Err(error) = run() {
        eprintln!("hyprmux-release: {error}");
        std::process::exit(2);
    }
}

fn run() -> Result<()> {
    let args = Arguments::parse(env::args().skip(1))?;
    if args.flag("help") {
        print_usage();
        return Ok(());
    }
    if args.command.is_none() {
        print_usage();
        return Ok(());
    }

    match args.command.as_deref() {
        Some("keygen") | Some("generate-key") => {
            args.ensure_options(&["id", "private-key", "public-key"], &[])?;
            if !args.positionals.is_empty() {
                return Err("keygen does not accept positional arguments".into());
            }
            generate_key(&args)
        }
        Some("manifest") | Some("generate-manifest") => {
            args.ensure_options(&["version", "output", "artifacts-dir", "archive"], &[])?;
            generate_manifest(&args)
        }
        Some("sign") | Some("sign-manifest") => {
            args.ensure_options(
                &["manifest", "output", "key-id", "private-key"],
                &["append"],
            )?;
            if !args.positionals.is_empty() {
                return Err("sign does not accept positional arguments".into());
            }
            sign_manifest(&args)
        }
        Some("verify") | Some("verify-release") => {
            args.ensure_options(&["manifest", "signatures", "keys", "artifacts-dir"], &[])?;
            if !args.positionals.is_empty() {
                return Err("verify does not accept positional arguments".into());
            }
            verify_release(&args)
        }
        Some("trust-check") => {
            args.ensure_options(&["keys"], &[])?;
            if !args.positionals.is_empty() {
                return Err("trust-check does not accept positional arguments".into());
            }
            trust_check(&args)
        }
        Some(other) => Err(format!("unknown command {other:?}; use --help")),
        None => Ok(()),
    }
}

fn print_usage() {
    println!(
        "\
hyprmux-release: release metadata and signing helper\n\n\
Commands:\n\
  keygen --id ID --private-key PATH --public-key PATH\n\
      Generate a fresh OS-random Ed25519 key pair. Both output paths are required and\n\
      existing files are never overwritten. Keep PATH to the private key outside the repository.\n\
  manifest --version VERSION --output PATH [--artifacts-dir DIR] [--archive PATH ...]\n\
      Inspect completed canonical archives, write hyprmux-release.json, and write adjacent .sha256\n\
      files from the final archive bytes.\n\
  sign --manifest PATH --output PATH --key-id ID [--private-key PATH] [--append]\n\
      Sign the exact manifest bytes. The private key is base64 in PATH, or in\n\
      HYPRMUX_RELEASE_PRIVATE_KEY when PATH is omitted.\n\
  verify --manifest PATH --signatures PATH --keys PATH --artifacts-dir DIR\n\
      Verify the rotation envelope, trusted public key, archive layout, payload hashes, sizes,\n\
      and adjacent checksums.\n\
  trust-check --keys PATH\n\
      Validate a committed public-key document and require at least one trusted key.\n\n\
The release workflow intentionally runs trust-check before the signing step. An empty trust\
document is a hard failure; generate and commit release-2026-a before publishing production tags.\n"
    );
}

#[derive(Debug, Default)]
struct Arguments {
    command: Option<String>,
    values: BTreeMap<String, Vec<String>>,
    flags: BTreeSet<String>,
    positionals: Vec<String>,
}

impl Arguments {
    fn parse<I>(mut input: I) -> Result<Self>
    where
        I: Iterator<Item = String>,
    {
        let mut result = Self::default();
        let Some(command) = input.next() else {
            return Ok(result);
        };
        if command == "--help" || command == "-h" {
            result.flags.insert("help".into());
            return Ok(result);
        }
        result.command = Some(command);

        let boolean_flags = ["append", "help"];
        let mut after_separator = false;
        while let Some(argument) = input.next() {
            if after_separator || !argument.starts_with('-') || argument == "-" {
                result.positionals.push(argument);
                continue;
            }
            if argument == "--" {
                after_separator = true;
                continue;
            }

            let raw = argument
                .strip_prefix("--")
                .ok_or_else(|| format!("unsupported argument {argument:?}; use long options"))?;
            let (name, inline) = raw
                .split_once('=')
                .map_or((raw, None), |(name, value)| (name, Some(value)));
            if name.is_empty() {
                return Err("empty option name".into());
            }
            if boolean_flags.contains(&name) {
                if inline.is_some() {
                    return Err(format!("--{name} does not take a value"));
                }
                result.flags.insert(name.to_string());
                continue;
            }
            let value = match inline {
                Some(value) if !value.is_empty() => value.to_string(),
                Some(_) => return Err(format!("--{name} requires a non-empty value")),
                None => input
                    .next()
                    .ok_or_else(|| format!("--{name} requires a value"))?,
            };
            if value.starts_with('-') && name != "private-key" {
                return Err(format!("--{name} requires a value, got {value:?}"));
            }
            result
                .values
                .entry(name.to_string())
                .or_default()
                .push(value);
        }
        Ok(result)
    }

    fn flag(&self, name: &str) -> bool {
        self.flags.contains(name)
    }

    fn one(&self, name: &str) -> Result<&str> {
        let values = self
            .values
            .get(name)
            .ok_or_else(|| format!("--{name} is required"))?;
        if values.len() != 1 {
            return Err(format!("--{name} must be supplied exactly once"));
        }
        Ok(values[0].as_str())
    }

    fn optional_one(&self, name: &str) -> Result<Option<&str>> {
        let Some(values) = self.values.get(name) else {
            return Ok(None);
        };
        if values.len() != 1 {
            return Err(format!("--{name} must be supplied at most once"));
        }
        Ok(Some(values[0].as_str()))
    }

    fn many(&self, name: &str) -> &[String] {
        self.values.get(name).map_or(&[], Vec::as_slice)
    }

    fn ensure_options(&self, values: &[&str], flags: &[&str]) -> Result<()> {
        for name in self.values.keys() {
            if !values.contains(&name.as_str()) {
                return Err(format!("unknown option --{name}"));
            }
        }
        for name in &self.flags {
            if name != "help" && !flags.contains(&name.as_str()) {
                return Err(format!("unknown flag --{name}"));
            }
        }
        Ok(())
    }
}

fn generate_key(args: &Arguments) -> Result<()> {
    let id = args.one("id")?;
    let private_path = Path::new(args.one("private-key")?);
    let public_path = Path::new(args.one("public-key")?);
    validate_key_id(id)?;
    if private_path == public_path {
        return Err("--private-key and --public-key must be different paths".into());
    }
    ensure_new_path(private_path, "private key")?;
    ensure_new_path(public_path, "public key")?;
    ensure_parent(private_path)?;
    ensure_parent(public_path)?;

    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(|error| format!("OS randomness failed: {error}"))?;
    let signing_key = SigningKey::from_bytes(&secret);

    let private_text = format!("{}\n", BASE64.encode(signing_key.to_bytes()));
    let public_document = TrustedKeySet {
        schema_version: signature::SIGNATURE_SCHEMA_VERSION,
        keys: vec![TrustedKey::ed25519(
            id,
            signing_key.verifying_key().to_bytes(),
        )],
    };
    let mut public_text = serde_json::to_vec_pretty(&public_document)
        .map_err(|error| format!("encode public keys: {error}"))?;
    public_text.push(b'\n');

    // The paths were checked before random generation and create_new is used for the final write,
    // so a maintainer cannot accidentally replace an existing private or trust file.
    write_new(private_path, private_text.as_bytes(), true)?;
    if let Err(error) = write_new(public_path, &public_text, false) {
        let _ = fs::remove_file(private_path);
        return Err(error);
    }
    eprintln!(
        "generated {id}; private key: {}, public document: {}",
        private_path.display(),
        public_path.display()
    );
    Ok(())
}

fn ensure_new_path(path: &Path, label: &str) -> Result<()> {
    if path.as_os_str().is_empty() {
        return Err(format!("{label} path is empty"));
    }
    if path.exists() {
        return Err(format!("refusing to overwrite {label} {}", path.display()));
    }
    Ok(())
}

fn ensure_parent(path: &Path) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        return Err(format!(
            "parent directory {} does not exist",
            parent.display()
        ));
    }
    Ok(())
}

fn write_new(path: &Path, bytes: &[u8], private: bool) -> Result<()> {
    #[cfg(not(unix))]
    let _ = private;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    } else {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o644);
    }
    let mut file = options
        .open(path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("write {}: {error}", path.display()))?;
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let mut permissions = file
            .metadata()
            .map_err(|error| format!("stat {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o600);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("protect {}: {error}", path.display()))?;
        let mode = fs::metadata(path)
            .map_err(|error| format!("stat {}: {error}", path.display()))?
            .mode();
        if mode & 0o077 != 0 {
            return Err(format!("private key {} is not mode 0600", path.display()));
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct ArtifactProbe {
    target: String,
    size: u64,
    sha256: String,
    payload: DigestInfo,
    launcher: Option<DigestInfo>,
}

fn generate_manifest(args: &Arguments) -> Result<()> {
    let version = args.one("version")?;
    let parsed_version = Target::parse_version(version).map_err(|error| error.to_string())?;
    let output = Path::new(args.one("output")?);
    let archives = collect_archives(args)?;
    let mut targets = BTreeMap::new();
    for archive in archives {
        let probe = inspect_archive(&archive, version)?;
        let target = Target::from_str(&probe.target).map_err(|error| error.to_string())?;
        let mut asset = ReleaseAsset::new(
            &parsed_version,
            target,
            probe.size,
            probe.sha256.clone(),
            probe.payload.size,
            probe.payload.sha256.clone(),
        );
        if target.is_windows() {
            let launcher = probe
                .launcher
                .as_ref()
                .ok_or_else(|| "Windows archive lacks launcher metadata".to_string())?;
            asset = asset.with_launcher(
                &parsed_version,
                target,
                target
                    .launcher_protocol()
                    .ok_or_else(|| "Windows target has no launcher protocol".to_string())?,
                launcher.size,
                launcher.sha256.clone(),
            );
        }
        let bytes =
            fs::read(&archive).map_err(|error| format!("read {}: {error}", archive.display()))?;
        release_archive::inspect_archive(&bytes, &asset).map_err(|error| error.to_string())?;
        write_checksum(&archive, &probe.sha256)?;
        if targets.insert(target, asset).is_some() {
            return Err(format!("duplicate release target {target}"));
        }
    }
    let published_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let release_manifest = ReleaseManifest::new(parsed_version, published_at, targets)
        .map_err(|error| error.to_string())?;
    let bytes = release_manifest
        .to_bytes()
        .map_err(|error| error.to_string())?;
    fs::write(output, bytes).map_err(|error| format!("write {}: {error}", output.display()))?;
    println!("wrote {}", output.display());
    Ok(())
}

fn collect_archives(args: &Arguments) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    if let Some(directory) = args.optional_one("artifacts-dir")? {
        let directory = Path::new(directory);
        let entries = fs::read_dir(directory).map_err(|error| {
            format!("read artifacts directory {}: {error}", directory.display())
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| format!("read artifact entry: {error}"))?;
            let path = entry.path();
            if path.is_file() && is_archive_name(&path) {
                paths.push(path);
            }
        }
    }
    paths.extend(args.many("archive").iter().map(PathBuf::from));
    paths.extend(args.positionals.iter().map(PathBuf::from));
    paths.sort();
    paths.dedup();
    if paths.is_empty() {
        return Err("no archives supplied; use --artifacts-dir or --archive".into());
    }
    Ok(paths)
}

fn is_archive_name(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.starts_with("hyprmux-") && (name.ends_with(".tar.gz") || name.ends_with(".zip"))
        })
}

fn inspect_archive(path: &Path, version: &str) -> Result<ArtifactProbe> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("archive path {} is not valid UTF-8", path.display()))?;
    if !is_archive_name(path) {
        return Err(format!("non-canonical archive name {name:?}"));
    }
    let prefix = format!("hyprmux-{version}-");
    let suffix = if name.ends_with(".zip") {
        ".zip"
    } else {
        ".tar.gz"
    };
    let stem = name
        .strip_suffix(suffix)
        .and_then(|stem| stem.strip_prefix(&prefix))
        .ok_or_else(|| format!("archive {name} does not match version {version}"))?;
    let target = stem.to_string();
    validate_target(&target)?;
    let is_windows = target_is_windows(&target);
    if is_windows != (suffix == ".zip") {
        return Err(format!(
            "archive {name} has the wrong format for target {target}"
        ));
    }
    let root = name.strip_suffix(suffix).unwrap_or(name);
    let archive_hash = hash_file(path)?;
    let expected_payload = if is_windows { "hyprmux.exe" } else { "hyprmux" };
    let expected_payload_path = format!("{root}/{expected_payload}");
    let expected_launcher_path = format!("{root}/hyprmux-launcher.exe");
    let contents = if is_windows {
        inspect_zip(path, root, &expected_payload_path, &expected_launcher_path)?
    } else {
        inspect_tar(path, root, &expected_payload_path, &expected_launcher_path)?
    };
    let launcher = contents.launcher;
    if is_windows && launcher.is_none() {
        return Err(format!("Windows archive {name} lacks hyprmux-launcher.exe"));
    }
    if !is_windows && launcher.is_some() {
        return Err(format!("non-Windows archive {name} contains a launcher"));
    }
    Ok(ArtifactProbe {
        target,
        size: archive_hash.size,
        sha256: archive_hash.sha256,
        payload: contents.payload,
        launcher,
    })
}

#[derive(Clone, Debug)]
struct ArchiveContents {
    payload: DigestInfo,
    launcher: Option<DigestInfo>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DigestInfo {
    size: u64,
    sha256: String,
}

fn inspect_tar(path: &Path, root: &str, payload: &str, launcher: &str) -> Result<ArchiveContents> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let decoder = GzDecoder::new(file);
    let mut archive = TarArchive::new(decoder);
    let mut paths = BTreeSet::new();
    let mut root_seen = false;
    let mut payload_digest = None;
    let mut launcher_digest = None;
    let entries = archive
        .entries()
        .map_err(|error| format!("read tar {}: {error}", path.display()))?;
    for entry in entries {
        let mut entry = entry.map_err(|error| format!("read tar entry: {error}"))?;
        let member = entry
            .path()
            .map_err(|error| format!("read tar member: {error}"))?
            .into_owned();
        let member = member
            .to_str()
            .ok_or_else(|| "tar member name is not UTF-8".to_string())?
            .to_string();
        validate_member(&member, root)?;
        if !paths.insert(member.clone()) {
            return Err(format!("archive contains duplicate member {member}"));
        }
        let entry_type = entry.header().entry_type();
        if entry_type.is_symlink() || entry_type.is_hard_link() {
            return Err(format!("archive contains link member {member}"));
        }
        if member.trim_end_matches('/') == root {
            root_seen = true;
        }
        if entry_type.is_file() {
            let digest = hash_reader(&mut entry)?;
            if member == payload {
                if entry.header().mode().unwrap_or(0) & 0o111 == 0 {
                    return Err(format!("payload {member} is not executable"));
                }
                payload_digest = Some(digest);
            } else if member == launcher {
                if entry.header().mode().unwrap_or(0) & 0o111 == 0 {
                    return Err(format!("launcher {member} is not executable"));
                }
                launcher_digest = Some(digest);
            }
        }
    }
    if !root_seen {
        return Err(format!("archive has no root directory {root}"));
    }
    Ok(ArchiveContents {
        payload: payload_digest.ok_or_else(|| format!("archive lacks payload {payload}"))?,
        launcher: launcher_digest,
    })
}

fn inspect_zip(path: &Path, root: &str, payload: &str, launcher: &str) -> Result<ArchiveContents> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut archive = ZipArchive::new(file).map_err(|error| format!("read zip: {error}"))?;
    let mut paths = BTreeSet::new();
    let mut root_seen = false;
    let mut payload_digest = None;
    let mut launcher_digest = None;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| format!("read zip entry {index}: {error}"))?;
        if entry.name().contains('\\') {
            return Err(format!("unsafe archive member {:?}", entry.name()));
        }
        let member = entry.name().to_string();
        validate_member(&member, root)?;
        if !paths.insert(member.clone()) {
            return Err(format!("archive contains duplicate member {member}"));
        }
        if entry.is_dir() {
            if member.trim_end_matches('/') == root {
                root_seen = true;
            }
            continue;
        }
        if member == root {
            return Err(format!("archive root {root} is not a directory"));
        }
        if let Some(mode) = entry.unix_mode() {
            if mode & 0o170000 == 0o120000 {
                return Err(format!("archive contains symlink member {member}"));
            }
            if (member == payload || member == launcher) && mode & 0o111 == 0 {
                return Err(format!("archive member {member} is not executable"));
            }
        }
        let digest = hash_reader(&mut entry)?;
        if member == payload {
            payload_digest = Some(digest);
        } else if member == launcher {
            launcher_digest = Some(digest);
        }
    }
    if !root_seen {
        return Err(format!("archive has no root directory {root}"));
    }
    Ok(ArchiveContents {
        payload: payload_digest.ok_or_else(|| format!("archive lacks payload {payload}"))?,
        launcher: launcher_digest,
    })
}

fn validate_member(member: &str, root: &str) -> Result<()> {
    let normalized = member.trim_end_matches('/');
    if member.contains('\\') || normalized.starts_with('/') || normalized.starts_with("../") {
        return Err(format!("unsafe archive member {member:?}"));
    }
    let path = Path::new(normalized);
    if path.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::CurDir | Component::RootDir
        )
    }) {
        return Err(format!("unsafe archive member {member:?}"));
    }
    let root_prefix = format!("{root}/");
    if normalized != root && !normalized.starts_with(&root_prefix) {
        return Err(format!("archive member {member:?} escapes root {root:?}"));
    }
    Ok(())
}

fn hash_reader(reader: &mut impl Read) -> Result<DigestInfo> {
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = reader
            .read(&mut buffer)
            .map_err(|error| format!("read archive payload: {error}"))?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
        size = size
            .checked_add(count as u64)
            .ok_or_else(|| "archive member is too large".to_string())?;
    }
    Ok(DigestInfo {
        size,
        sha256: hex::encode(hasher.finalize()),
    })
}

fn hash_file(path: &Path) -> Result<DigestInfo> {
    let file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let metadata = file
        .metadata()
        .map_err(|error| format!("stat {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("{} is not a regular file", path.display()));
    }
    let sha256 = sha256_file(path).map_err(|error| format!("hash {}: {error}", path.display()))?;
    let after = fs::metadata(path).map_err(|error| format!("stat {}: {error}", path.display()))?;
    if after.len() != metadata.len() {
        return Err(format!("{} changed while hashing", path.display()));
    }
    Ok(DigestInfo {
        size: metadata.len(),
        sha256,
    })
}

fn write_checksum(path: &Path, digest: &str) -> Result<()> {
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid archive name {}", path.display()))?;
    let checksum = path.with_file_name(format!("{name}.sha256"));
    fs::write(&checksum, format!("{digest}  {name}\n"))
        .map_err(|error| format!("write {}: {error}", checksum.display()))
}

fn sign_manifest(args: &Arguments) -> Result<()> {
    let manifest_path = Path::new(args.one("manifest")?);
    let output = Path::new(args.one("output")?);
    let key_id = args.one("key-id")?;
    validate_key_id(key_id)?;
    let manifest_bytes = fs::read(manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let _manifest = ReleaseManifest::from_bytes(&manifest_bytes)
        .map_err(|error| format!("parse manifest {}: {error}", manifest_path.display()))?;
    validate_published_at(&_manifest.published_at)?;
    let private = load_private_key(args)?;
    let mut envelope = if args.flag("append") && output.exists() {
        let bytes =
            fs::read(output).map_err(|error| format!("read {}: {error}", output.display()))?;
        SignatureEnvelope::from_bytes(&bytes)
            .map_err(|error| format!("parse {}: {error}", output.display()))?
    } else {
        SignatureEnvelope::new(Vec::new())
    };
    let new_signature = signature::sign_manifest(&manifest_bytes, key_id, &private);
    if envelope
        .signatures
        .iter()
        .any(|record| record.key_id == key_id)
    {
        return Err(format!("signature for key {key_id} already exists"));
    }
    envelope.signatures.extend(new_signature.signatures);
    envelope
        .signatures
        .sort_by(|left, right| left.key_id.cmp(&right.key_id));
    fs::write(
        output,
        envelope.to_bytes().map_err(|error| error.to_string())?,
    )
    .map_err(|error| format!("write {}: {error}", output.display()))?;
    println!("signed {}", manifest_path.display());
    Ok(())
}

fn load_private_key(args: &Arguments) -> Result<SigningKey> {
    let encoded = if let Some(path) = args.optional_one("private-key")? {
        let bytes = fs::read(path).map_err(|error| format!("read private key {path}: {error}"))?;
        String::from_utf8(bytes).map_err(|_| "private key is not UTF-8 base64".to_string())?
    } else {
        env::var("HYPRMUX_RELEASE_PRIVATE_KEY")
            .map_err(|_| "--private-key or HYPRMUX_RELEASE_PRIVATE_KEY is required".to_string())?
    };
    let encoded = encoded.trim();
    if encoded.is_empty() || encoded.chars().any(char::is_whitespace) {
        return Err("private key must be one base64 value".into());
    }
    let bytes = BASE64
        .decode(encoded)
        .map_err(|error| format!("decode private key: {error}"))?;
    let bytes: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "private key must decode to exactly 32 bytes".to_string())?;
    Ok(SigningKey::from_bytes(&bytes))
}

fn verify_release(args: &Arguments) -> Result<()> {
    let manifest_path = Path::new(args.one("manifest")?);
    let signatures_path = Path::new(args.one("signatures")?);
    let keys_path = Path::new(args.one("keys")?);
    let artifacts_dir = Path::new(args.one("artifacts-dir")?);
    if manifest_path.file_name().and_then(|name| name.to_str()) != Some(MANIFEST_NAME) {
        return Err(format!("manifest must be named {MANIFEST_NAME}"));
    }
    if signatures_path.file_name().and_then(|name| name.to_str()) != Some(SIGNATURES_NAME) {
        return Err(format!("signatures must be named {SIGNATURES_NAME}"));
    }
    let manifest_bytes = fs::read(manifest_path)
        .map_err(|error| format!("read {}: {error}", manifest_path.display()))?;
    let manifest = ReleaseManifest::from_bytes(&manifest_bytes)
        .map_err(|error| format!("parse manifest {}: {error}", manifest_path.display()))?;
    validate_published_at(&manifest.published_at)?;
    let signatures_bytes = fs::read(signatures_path)
        .map_err(|error| format!("read {}: {error}", signatures_path.display()))?;
    let _envelope = SignatureEnvelope::from_bytes(&signatures_bytes)
        .map_err(|error| format!("parse signatures {}: {error}", signatures_path.display()))?;
    let keys = read_keys(keys_path, true)?;
    signature::verify_manifest_with_keys(&manifest_bytes, &signatures_bytes, &keys.keys)
        .map_err(|error| error.to_string())?;
    verify_artifacts(&manifest, artifacts_dir)?;
    println!("verified {} ({})", manifest.version, manifest.targets.len());
    Ok(())
}

fn trust_check(args: &Arguments) -> Result<()> {
    let path = Path::new(args.one("keys")?);
    let keys = read_keys(path, true).map_err(|error| {
        format!(
            "{error}; generate and commit release-2026-a before using a production signing secret"
        )
    })?;
    println!(
        "trusted keys: {}",
        keys.keys
            .iter()
            .map(|key| key.id.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    Ok(())
}

fn read_keys(path: &Path, require_nonempty: bool) -> Result<TrustedKeySet> {
    let bytes = fs::read(path).map_err(|error| format!("read {}: {error}", path.display()))?;
    let document = TrustedKeySet::from_bytes(&bytes)
        .map_err(|error| format!("parse public key document {}: {error}", path.display()))?;
    if require_nonempty && !document.has_ed25519_key() {
        return Err(format!(
            "public key document {} contains no trusted keys",
            path.display()
        ));
    }
    for key in &document.keys {
        validate_key_id(&key.id)?;
        if key.algorithm != PUBLIC_KEY_ALGORITHM {
            return Err(format!(
                "key {} uses unsupported algorithm {}",
                key.id, key.algorithm
            ));
        }
    }
    Ok(document)
}

fn verify_artifacts(manifest: &ReleaseManifest, directory: &Path) -> Result<()> {
    if !directory.is_dir() {
        return Err(format!(
            "artifacts directory {} does not exist",
            directory.display()
        ));
    }
    let mut expected = BTreeSet::new();
    for (target, expected_asset) in &manifest.targets {
        if !expected.insert(expected_asset.archive.clone()) {
            return Err(format!(
                "manifest repeats archive {}",
                expected_asset.archive
            ));
        }
        let path = directory.join(&expected_asset.archive);
        let bytes = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        release_archive::inspect_archive(&bytes, expected_asset)
            .map_err(|error| format!("verify {}: {error}", path.display()))?;
        if bytes.len() as u64 != expected_asset.archive_size {
            return Err(format!(
                "manifest size does not match final archive {} ({target})",
                path.display()
            ));
        }
        verify_checksum_file(&path, &expected_asset.archive_sha256)?;
    }
    let entries = fs::read_dir(directory)
        .map_err(|error| format!("read artifacts directory {}: {error}", directory.display()))?;
    for entry in entries {
        let path = entry
            .map_err(|error| format!("read artifact entry: {error}"))?
            .path();
        if is_archive_name(&path) {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("invalid artifact path {}", path.display()))?;
            if !expected.contains(name) {
                return Err(format!("unlisted archive in artifacts directory: {name}"));
            }
        }
    }
    Ok(())
}

fn verify_checksum_file(archive: &Path, expected: &str) -> Result<()> {
    let name = archive
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("invalid archive path {}", archive.display()))?;
    let checksum_path = archive.with_file_name(format!("{name}.sha256"));
    let text = fs::read_to_string(&checksum_path)
        .map_err(|error| format!("read {}: {error}", checksum_path.display()))?;
    let mut fields = text.split_whitespace();
    let actual = fields
        .next()
        .ok_or_else(|| format!("empty checksum {}", checksum_path.display()))?;
    let listed = fields
        .next()
        .ok_or_else(|| format!("malformed checksum {}", checksum_path.display()))?
        .trim_start_matches('*');
    if fields.next().is_some() || !is_lower_hex(actual, 64) || listed != name || actual != expected
    {
        return Err(format!(
            "checksum {} does not match {name}",
            checksum_path.display()
        ));
    }
    let computed = hash_file(archive)?.sha256;
    if computed != expected {
        return Err(format!("archive {name} changed after manifest generation"));
    }
    Ok(())
}

fn validate_target(target: &str) -> Result<()> {
    Target::from_str(target)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn validate_published_at(value: &str) -> Result<()> {
    let parsed = DateTime::parse_from_rfc3339(value)
        .map_err(|error| format!("invalid published_at timestamp: {error}"))?;
    let canonical = parsed
        .with_timezone(&Utc)
        .to_rfc3339_opts(SecondsFormat::Secs, true);
    if canonical != value {
        return Err(format!(
            "published_at must be canonical UTC RFC3339 with second precision: {value}"
        ));
    }
    Ok(())
}

fn target_is_windows(target: &str) -> bool {
    Target::from_str(target).is_ok_and(Target::is_windows)
}

fn validate_key_id(id: &str) -> Result<()> {
    if id.is_empty()
        || id.len() > 64
        || !id.as_bytes()[0].is_ascii_alphanumeric()
        || id.contains("..")
        || id.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(format!("invalid key id {id:?}"));
    }
    Ok(())
}

fn is_lower_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_names_reject_wrong_windows_format() {
        let path = Path::new("hyprmux-1.2.3-x86_64-pc-windows-msvc.tar.gz");
        let error = inspect_archive(path, "1.2.3")
            .expect_err("missing archive should still be rejected by name");
        assert!(error.contains("wrong format") || error.contains("open"));
    }

    #[test]
    fn key_ids_do_not_allow_paths() {
        assert!(validate_key_id("release-2026-a").is_ok());
        assert!(validate_key_id("../private").is_err());
        assert!(validate_key_id("release/a").is_err());
    }

    #[test]
    fn hashes_are_lowercase() {
        assert!(is_lower_hex(&"00".repeat(32), 64));
        assert!(!is_lower_hex(&"AA".repeat(32), 64));
    }

    #[test]
    fn published_at_requires_canonical_utc_seconds() {
        assert!(validate_published_at("2026-08-02T12:34:56Z").is_ok());
        assert!(validate_published_at("2026-08-02T12:34:56+00:00").is_err());
        assert!(validate_published_at("2026-08-02T12:34:56.123Z").is_err());
    }
}
