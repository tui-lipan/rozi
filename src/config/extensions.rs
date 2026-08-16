use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Serialize;

use super::schema::{NamedCommand, ServiceConfig};

mod contributions;
mod diagnostics;
mod discovery;
mod manifest;
mod paths;
mod runtime;
mod validation;

pub use diagnostics::{
    EXTENSION_DIAGNOSTICS_SCHEMA_VERSION, ExtensionCheckDocument, ExtensionListDocument,
};
use manifest::{ExtensionManifestFile, ExtensionSettingsOnly};
use paths::{absolute_path, normalize_path};
pub use runtime::{ExtensionProvenance, GENERATION_ENV};
pub(crate) use runtime::{
    ExtensionRuntimeFingerprint, fingerprint, fingerprints_by_id, provenance_from_process,
    provenance_is_active, reconcile_generations,
};
pub(crate) use validation::is_extension_command_id;
use validation::{validate_command, validate_extension_id, validate_service};

/// The generation of Rozi's complete public extension contract.
///
/// Internal Rust APIs may change independently. Manifests, injected environment, command and
/// service behavior, extension-facing control commands, and lifecycle guarantees move together.
pub const EXTENSION_API_VERSION: u32 = 1;

const RESERVED_EXTENSION_IDS: &[&str] = &["app", "command", "rozi", "user", "workspace"];
const RESERVED_EXTENSION_ENV: &[&str] = &[
    "ROZI_EXTENSION",
    "ROZI_EXTENSION_DIR",
    runtime::GENERATION_ENV,
];

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExtensionStatus {
    Loaded,
    Disabled,
    Invalid,
    Incompatible,
    Duplicate,
}

impl ExtensionStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::Disabled => "disabled",
            Self::Invalid => "invalid",
            Self::Incompatible => "incompatible",
            Self::Duplicate => "duplicate",
        }
    }
}

/// Stable diagnostic representation used by both human and JSON CLI output.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionInfo {
    pub id: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub version: Option<String>,
    pub api: Option<u32>,
    pub path: String,
    pub manifest_path: String,
    pub enabled: bool,
    pub status: ExtensionStatus,
    pub commands: Vec<String>,
    pub services: Vec<String>,
    pub command_paths: BTreeMap<String, String>,
    pub service_paths: BTreeMap<String, String>,
    pub errors: Vec<String>,
}

impl ExtensionInfo {
    pub fn display_name(&self) -> &str {
        self.id
            .as_deref()
            .or(self.title.as_deref())
            .or_else(|| {
                Path::new(&self.path)
                    .file_name()
                    .and_then(|name| name.to_str())
            })
            .unwrap_or("<unknown>")
    }

    pub fn status_detail(&self) -> String {
        match (self.status, self.errors.first()) {
            (ExtensionStatus::Loaded | ExtensionStatus::Disabled, _) | (_, None) => {
                self.status.as_str().to_string()
            }
            (status, Some(error)) => format!("{}: {error}", status.as_str()),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct DiscoveredExtension {
    pub(crate) info: ExtensionInfo,
    commands: Vec<NamedCommand>,
    services: Vec<ServiceConfig>,
}

#[derive(Debug, Default)]
pub(crate) struct ExtensionScan {
    pub(crate) extensions: Vec<DiscoveredExtension>,
    pub(crate) root_errors: Vec<String>,
}

impl ExtensionScan {
    pub(crate) fn entries(&self) -> Vec<ExtensionInfo> {
        self.extensions
            .iter()
            .map(|extension| extension.info.clone())
            .collect()
    }

    pub(crate) fn apply_disabled(&mut self, disabled: &[String]) {
        let disabled: HashSet<_> = disabled.iter().map(|id| id.trim()).collect();
        for extension in &mut self.extensions {
            if extension.info.status == ExtensionStatus::Loaded
                && extension
                    .info
                    .id
                    .as_deref()
                    .is_some_and(|id| disabled.contains(id))
            {
                extension.info.status = ExtensionStatus::Disabled;
                extension.info.enabled = false;
            }
        }
    }

    pub(crate) fn into_contributions(
        self,
        disabled: &[String],
    ) -> contributions::ExtensionContributions {
        contributions::build(self, disabled)
    }
}

pub(crate) fn scan_extensions() -> ExtensionScan {
    let env = crate::platform::paths::PlatformEnv::from_process();
    scan_extensions_in(&crate::platform::paths::extensions_dir(&env))
}

pub(crate) fn scan_extensions_for_cli() -> ExtensionScan {
    let mut scan = scan_extensions();
    let disabled = std::fs::read_to_string(super::config_path())
        .ok()
        .and_then(|text| toml::from_str::<ExtensionSettingsOnly>(&text).ok())
        .map(|settings| settings.extensions.disabled)
        .unwrap_or_default();
    scan.apply_disabled(&disabled);
    scan
}

/// Validate one extension directory without installing it.
pub(crate) fn check_extension(path: &Path) -> DiscoveredExtension {
    let directory = if path
        .file_name()
        .is_some_and(|name| name == "extension.toml")
    {
        path.parent().unwrap_or_else(|| Path::new("."))
    } else {
        path
    };
    build_candidate(&absolute_path(directory))
}

pub(crate) fn scan_extensions_in(root: &Path) -> ExtensionScan {
    let (directories, root_errors) = discovery::directories(root);
    let mut scan = ExtensionScan {
        extensions: Vec::new(),
        root_errors,
    };
    scan.extensions = directories
        .into_iter()
        .map(|directory| build_candidate(&absolute_path(&directory)))
        .collect();
    discovery::mark_duplicate_ids(&mut scan.extensions);
    scan
}

fn build_candidate(directory: &Path) -> DiscoveredExtension {
    let directory = normalize_path(directory);
    let path = directory.to_string_lossy().to_string();
    let manifest_path_buf = directory.join("extension.toml");
    let manifest_path = manifest_path_buf.to_string_lossy().to_string();
    let mut info = ExtensionInfo {
        id: None,
        title: None,
        description: None,
        version: None,
        api: None,
        path,
        manifest_path,
        enabled: false,
        status: ExtensionStatus::Invalid,
        commands: Vec::new(),
        services: Vec::new(),
        command_paths: BTreeMap::new(),
        service_paths: BTreeMap::new(),
        errors: Vec::new(),
    };
    if directory.to_str().is_none() {
        info.errors.push(
            "installation path is not valid UTF-8 and cannot be exported in the extension environment"
                .to_string(),
        );
    }

    let manifest_text = match std::fs::read_to_string(&manifest_path_buf) {
        Ok(text) => text,
        Err(error) => {
            info.errors
                .push(format!("could not read extension.toml: {error}"));
            return DiscoveredExtension {
                info,
                commands: Vec::new(),
                services: Vec::new(),
            };
        }
    };
    let value: toml::Value = match toml::from_str(&manifest_text) {
        Ok(value) => value,
        Err(error) => {
            info.errors.push(format!("invalid extension.toml: {error}"));
            return DiscoveredExtension {
                info,
                commands: Vec::new(),
                services: Vec::new(),
            };
        }
    };
    read_partial_metadata(&value, &mut info);
    let manifest: ExtensionManifestFile = match toml::from_str(&manifest_text) {
        Ok(manifest) => manifest,
        Err(error) => {
            info.errors.push(format!("invalid extension.toml: {error}"));
            return DiscoveredExtension {
                info,
                commands: Vec::new(),
                services: Vec::new(),
            };
        }
    };

    info.id = clean_optional(manifest.extension.id);
    info.title = clean_optional(manifest.extension.title);
    info.description = clean_optional(manifest.extension.description);
    info.version = clean_optional(manifest.extension.version);
    info.api = manifest.extension.api;

    let id_valid = validate_extension_id(info.id.as_deref(), &mut info.errors);
    let compatibility_error = match info.api {
        None => {
            info.errors
                .push("missing required field `extension.api`".to_string());
            None
        }
        Some(api) if api != EXTENSION_API_VERSION => Some(format!(
            "requires extension API {api}, rozi supports API {EXTENSION_API_VERSION}"
        )),
        Some(_) => None,
    };

    let mut commands = Vec::new();
    let mut services = Vec::new();
    let validation_id = info
        .id
        .clone()
        .filter(|_| id_valid)
        .unwrap_or_else(|| "extension".to_string());
    let category = info.title.clone().unwrap_or_else(|| validation_id.clone());
    let extension_dir = directory.to_string_lossy().to_string();
    let command_env = vec![
        ("ROZI_EXTENSION".to_string(), validation_id.clone()),
        ("ROZI_EXTENSION_DIR".to_string(), extension_dir.clone()),
    ];
    let mut seen_commands = HashSet::new();
    for raw in manifest.commands {
        validate_command(
            raw,
            &validation_id,
            &category,
            &directory,
            &command_env,
            &mut seen_commands,
            &mut info,
            &mut commands,
        );
    }
    let mut seen_services = HashSet::new();
    for raw in manifest.services {
        validate_service(
            raw,
            &validation_id,
            &directory,
            &extension_dir,
            &mut seen_services,
            &mut info,
            &mut services,
        );
    }
    if !id_valid {
        info.commands.clear();
        info.services.clear();
        info.command_paths.clear();
        info.service_paths.clear();
    }

    if let Some(error) = compatibility_error {
        info.status = ExtensionStatus::Incompatible;
        info.errors.insert(0, error);
        commands.clear();
        services.clear();
    } else if !info.errors.is_empty() {
        info.status = ExtensionStatus::Invalid;
        commands.clear();
        services.clear();
    } else {
        info.status = ExtensionStatus::Loaded;
        info.enabled = true;
    }
    DiscoveredExtension {
        info,
        commands,
        services,
    }
}

fn read_partial_metadata(value: &toml::Value, info: &mut ExtensionInfo) {
    let Some(extension) = value.get("extension").and_then(toml::Value::as_table) else {
        return;
    };
    info.id = extension
        .get("id")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    info.title = extension
        .get("title")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    info.description = extension
        .get("description")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    info.version = extension
        .get("version")
        .and_then(toml::Value::as_str)
        .map(str::to_string);
    info.api = extension
        .get("api")
        .and_then(toml::Value::as_integer)
        .and_then(|api| u32::try_from(api).ok());
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServiceLaunch;
    use std::path::PathBuf;

    fn write_manifest(root: &Path, directory: &str, text: &str) -> PathBuf {
        let dir = root.join(directory);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("extension.toml"), text).unwrap();
        dir
    }

    fn write_program(directory: &Path, relative: &str) {
        let path = directory.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "exit 0\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }
    }

    fn manifest(id: &str, api: &str) -> String {
        format!(
            "[extension]\nid = \"{id}\"\ntitle = \"Git tools\"\nversion = \"0.1.0\"\napi = {api}\n"
        )
    }

    #[test]
    fn stable_manifest_id_is_independent_of_installation_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(
            temp.path(),
            "rozi-git-tools",
            &format!(
                "{}[[commands]]\nid = \"branches\"\nexec = [\"./bin/branches\", \"--all\"]\n\
                 [[services]]\nname = \"watch\"\nexec = [\"./bin/watch\"]\n",
                manifest("git-tools", "1")
            ),
        );
        write_program(&directory, "bin/branches");
        write_program(&directory, "bin/watch");

        let scan = scan_extensions_in(temp.path());
        assert_eq!(scan.extensions.len(), 1);
        let entries = scan.entries();
        assert_eq!(entries[0].id.as_deref(), Some("git-tools"));
        assert_eq!(entries[0].status, ExtensionStatus::Loaded);
        assert_eq!(entries[0].commands, ["git-tools.branches"]);
        assert_eq!(entries[0].services, ["git-tools.watch"]);
        assert_eq!(entries[0].path, directory.display().to_string());

        let (commands, services, active, runtime, warnings) = scan.into_contributions(&[]);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(active.contains("git-tools"));
        assert!(runtime.contains_key("git-tools"));
        assert_eq!(commands[0].id, "git-tools.branches");
        assert_eq!(commands[0].category, "Git tools");
        assert!(
            commands[0]
                .env
                .iter()
                .any(|(key, value)| key == "ROZI_EXTENSION" && value == "git-tools")
        );
        let command = match &commands[0].action {
            super::super::schema::UserCommandAction::ExecDirect { argv } => argv,
            other => panic!("unexpected action: {other:?}"),
        };
        assert!(
            command[0].contains("/rozi-git-tools/bin/branches"),
            "{command:?}"
        );
        assert_eq!(services[0].name, "git-tools.watch");
        assert_eq!(
            services[0].cwd.as_deref(),
            Some(directory.display().to_string().as_str())
        );
        assert!(matches!(
            &services[0].launch,
            ServiceLaunch::Direct(argv) if argv[0].contains("/rozi-git-tools/bin/watch")
        ));
        assert_eq!(
            services[0].env.get("ROZI_EXTENSION").map(String::as_str),
            Some("git-tools")
        );
        assert_eq!(
            services[0]
                .env
                .get("ROZI_EXTENSION_DIR")
                .map(String::as_str),
            Some(directory.display().to_string().as_str())
        );
    }

    #[test]
    fn every_api_compatibility_case_is_diagnostic_and_non_contributing() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(temp.path(), "missing", &manifest("missing", "\"oops\""));
        write_manifest(temp.path(), "old", &manifest("old", "0"));
        write_manifest(temp.path(), "future", &manifest("future", "2"));
        write_manifest(
            temp.path(),
            "no-api",
            "[extension]\nid = \"no-api\"\nversion = \"1.0.0\"\n",
        );

        let scan = scan_extensions_in(temp.path());
        let entries = scan.entries();
        let by_id = |id: &str| {
            entries
                .iter()
                .find(|entry| entry.id.as_deref() == Some(id))
                .unwrap()
        };
        assert_eq!(by_id("old").status, ExtensionStatus::Incompatible);
        assert_eq!(by_id("future").status, ExtensionStatus::Incompatible);
        assert!(by_id("old").errors[0].contains("supports API 1"));
        assert_eq!(by_id("no-api").status, ExtensionStatus::Invalid);
        assert!(by_id("no-api").errors[0].contains("extension.api"));
        let malformed = entries
            .iter()
            .find(|entry| entry.path.ends_with("/missing"))
            .unwrap();
        assert_eq!(malformed.status, ExtensionStatus::Invalid);
        assert!(malformed.errors[0].contains("invalid extension.toml"));
        let (commands, services, active, _, _) = scan.into_contributions(&[]);
        assert!(commands.is_empty());
        assert!(services.is_empty());
        assert!(active.is_empty());
    }

    #[test]
    fn invalid_and_reserved_ids_are_rejected() {
        for (index, id) in ["", "Bad", "has.dot", "has space", "../escape", "rozi"]
            .into_iter()
            .enumerate()
        {
            let temp = tempfile::tempdir().unwrap();
            write_manifest(
                temp.path(),
                &format!("candidate-{index}"),
                &manifest(id, "1"),
            );
            let entry = scan_extensions_in(temp.path()).entries().remove(0);
            assert_eq!(entry.status, ExtensionStatus::Invalid, "{id:?}");
            assert!(!entry.errors.is_empty(), "{id:?}");
        }
    }

    #[test]
    fn duplicate_ids_invalidate_every_ambiguous_installation() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(temp.path(), "one", &manifest("same", "1"));
        write_manifest(temp.path(), "two", &manifest("same", "1"));
        let scan = scan_extensions_in(temp.path());
        assert!(scan.entries().iter().all(|entry| {
            entry.status == ExtensionStatus::Duplicate
                && entry.errors[0].contains("/one")
                && entry.errors[0].contains("/two")
        }));
        let (commands, services, active, _, _) = scan.into_contributions(&[]);
        assert!(commands.is_empty());
        assert!(services.is_empty());
        assert!(active.is_empty());
    }

    #[test]
    fn disabled_extension_remains_visible_but_contributes_nothing() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(temp.path(), "docker-install", &manifest("docker", "1"));
        let mut scan = scan_extensions_in(temp.path());
        scan.apply_disabled(&["docker".to_string()]);
        assert_eq!(scan.entries()[0].status, ExtensionStatus::Disabled);
        let (commands, services, active, _, warnings) =
            scan.into_contributions(&["docker".to_string()]);
        assert!(commands.is_empty());
        assert!(services.is_empty());
        assert!(active.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn one_bad_extension_does_not_hide_sorted_neighbors() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(temp.path(), "z-good", &manifest("z-good", "1"));
        write_manifest(temp.path(), "a-broken", "not toml");
        write_manifest(temp.path(), "m-good", &manifest("m-good", "1"));
        let entries = scan_extensions_in(temp.path()).entries();
        assert!(entries[0].path.ends_with("/a-broken"));
        assert!(entries[1].path.ends_with("/m-good"));
        assert!(entries[2].path.ends_with("/z-good"));
        assert_eq!(entries[0].status, ExtensionStatus::Invalid);
        assert_eq!(entries[1].status, ExtensionStatus::Loaded);
        assert_eq!(entries[2].status, ExtensionStatus::Loaded);
    }

    #[test]
    fn duplicate_members_reserved_env_and_missing_paths_invalidate_atomically() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "bad",
            &format!(
                "{}[[commands]]\nid = \"same\"\nexec = [\"./missing\"]\n\
                 [[commands]]\nid = \"same\"\nsend = \"x\"\n\
                 [[services]]\nname = \"watch\"\nexec = [\"./missing-service\"]\n\
                 [services.env]\nROZI_EXTENSION = \"spoof\"\n\
                 [[services]]\nname = \"watch\"\nshell = \"echo ok\"\n",
                manifest("bad-tools", "1")
            ),
        );
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        let errors = entry.errors.join("\n");
        assert!(errors.contains("duplicate command"));
        assert!(errors.contains("duplicate service"));
        assert!(errors.contains("reserved environment"));
        assert!(errors.contains("unavailable"));
    }

    #[cfg(unix)]
    #[test]
    fn declared_exec_requires_an_executable_file() {
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(
            temp.path(),
            "not-executable",
            &format!(
                "{}[[commands]]\nid = \"open\"\nexec = [\"./bin/open\"]\n",
                manifest("tools", "1")
            ),
        );
        let program = directory.join("bin/open");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(program, "exit 0\n").unwrap();

        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(
            entry
                .errors
                .iter()
                .any(|error| error.contains("is not executable"))
        );
    }

    #[test]
    fn moving_an_installation_keeps_public_identity_and_updates_environment() {
        let temp = tempfile::tempdir().unwrap();
        let first = write_manifest(temp.path(), "first-name", &manifest("movable", "1"));
        let before = scan_extensions_in(temp.path()).entries().remove(0);
        let second = temp.path().join("second-name");
        std::fs::rename(&first, &second).unwrap();
        let after = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(before.id, after.id);
        assert_eq!(before.status, ExtensionStatus::Loaded);
        assert_eq!(after.status, ExtensionStatus::Loaded);
        assert_ne!(before.path, after.path);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_installations_use_the_lexical_extension_directory() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let payload = tempfile::tempdir().unwrap();
        write_manifest(payload.path(), "payload", &manifest("linked", "1"));
        let link = temp.path().join("friendly-name");
        symlink(payload.path().join("payload"), &link).unwrap();
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Loaded);
        assert_eq!(entry.path, link.display().to_string());
    }

    #[cfg(unix)]
    #[test]
    fn broken_symlink_and_non_utf8_installation_are_diagnostic_candidates() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        symlink(
            temp.path().join("missing-target"),
            temp.path().join("broken"),
        )
        .unwrap();
        let non_utf8 = temp
            .path()
            .join(OsString::from_vec(b"non-utf8-\xff".to_vec()));
        std::fs::create_dir_all(&non_utf8).unwrap();
        std::fs::write(non_utf8.join("extension.toml"), manifest("non-utf8", "1")).unwrap();

        let entries = scan_extensions_in(temp.path()).entries();
        assert_eq!(entries.len(), 2);
        assert!(
            entries
                .iter()
                .all(|entry| entry.status == ExtensionStatus::Invalid)
        );
        assert!(
            entries
                .iter()
                .flat_map(|entry| &entry.errors)
                .any(|error| error.contains("not valid UTF-8"))
        );
        assert!(
            entries
                .iter()
                .flat_map(|entry| &entry.errors)
                .any(|error| error.contains("could not read extension.toml"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_manifest_is_reported_when_permissions_are_enforced() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(temp.path(), "private", &manifest("private", "1"));
        let manifest_path = directory.join("extension.toml");
        std::fs::set_permissions(&manifest_path, std::fs::Permissions::from_mode(0o0)).unwrap();
        if std::fs::read_to_string(&manifest_path).is_err() {
            let entry = scan_extensions_in(temp.path()).entries().remove(0);
            assert_eq!(entry.status, ExtensionStatus::Invalid);
            assert!(entry.errors[0].contains("could not read extension.toml"));
        }
        std::fs::set_permissions(manifest_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn windows_relative_program_paths_resolve_against_installation_directory() {
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(
            temp.path(),
            "windows",
            &format!(
                "{}[[commands]]\nid = \"open\"\nexec = [\".\\\\bin\\\\open.cmd\"]\n",
                manifest("windows-tools", "1")
            ),
        );
        write_program(&directory, "bin/open.cmd");
        let info = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(info.status, ExtensionStatus::Loaded);
        assert!(info.command_paths["windows-tools.open"].ends_with(r"bin\open.cmd"));
    }

    #[test]
    fn missing_root_is_an_empty_success() {
        let temp = tempfile::tempdir().unwrap();
        let scan = scan_extensions_in(&temp.path().join("not-created"));
        assert!(scan.extensions.is_empty());
        assert!(scan.root_errors.is_empty());
    }

    #[test]
    fn canonical_example_extensions_validate_as_third_party_checkouts() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/extensions");
        for directory in ["git-tools", "activity-dashboard"] {
            let extension = check_extension(&root.join(directory));
            assert_eq!(
                extension.info.status,
                ExtensionStatus::Loaded,
                "{directory}: {:?}",
                extension.info.errors
            );
        }
    }

    #[test]
    fn golden_manifest_fixtures_cover_valid_invalid_and_api_compatibility_contracts() {
        let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/extensions");
        for (bucket, expected) in [
            ("valid", ExtensionStatus::Loaded),
            ("invalid", ExtensionStatus::Invalid),
        ] {
            for entry in std::fs::read_dir(fixtures.join(bucket)).unwrap() {
                let path = entry.unwrap().path();
                let info = check_extension(&path).info;
                if path.ends_with("incompatible-api") {
                    assert_eq!(info.status, ExtensionStatus::Incompatible, "{path:?}");
                } else {
                    assert_eq!(info.status, expected, "{path:?}: {:?}", info.errors);
                }
            }
        }

        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/extension.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            schema["properties"]["extension"]["properties"]["api"]["const"],
            EXTENSION_API_VERSION
        );
    }

    #[test]
    fn diagnostic_json_exposes_public_ids_and_structured_status() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/extensions");
        let info = check_extension(&root.join("git-tools")).info;
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json["status"], "loaded");
        assert_eq!(json["enabled"], true);
        assert_eq!(json["api"], EXTENSION_API_VERSION);
        assert_eq!(json["commands"][0], "git-tools.branches");
        assert!(!json.to_string().contains("command.git-tools"));
    }

    #[test]
    fn discovery_transition_matrix_has_exact_enabled_contributions() {
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(
            temp.path(),
            "install-a",
            &format!(
                "{}[[commands]]\nid = \"one\"\nsend = \"one\"\n",
                manifest("matrix", "1")
            ),
        );
        let loaded = scan_extensions_in(temp.path());
        assert_eq!(loaded.entries()[0].status, ExtensionStatus::Loaded);
        assert_eq!(loaded.entries()[0].commands, ["matrix.one"]);

        std::fs::write(
            directory.join("extension.toml"),
            format!(
                "{}[[commands]]\nid = \"two\"\nsend = \"changed\"\n",
                manifest("matrix", "1")
            ),
        )
        .unwrap();
        let changed = scan_extensions_in(temp.path());
        assert_eq!(changed.entries()[0].commands, ["matrix.two"]);

        let (commands, _, active, _, _) =
            scan_extensions_in(temp.path()).into_contributions(&["matrix".to_string()]);
        assert!(commands.is_empty());
        assert!(active.is_empty());

        std::fs::write(directory.join("extension.toml"), "not toml").unwrap();
        assert_eq!(
            scan_extensions_in(temp.path()).entries()[0].status,
            ExtensionStatus::Invalid
        );

        std::fs::write(directory.join("extension.toml"), manifest("matrix", "2")).unwrap();
        assert_eq!(
            scan_extensions_in(temp.path()).entries()[0].status,
            ExtensionStatus::Incompatible
        );

        std::fs::write(
            directory.join("extension.toml"),
            manifest("renamed-id", "1"),
        )
        .unwrap();
        assert_eq!(
            scan_extensions_in(temp.path()).entries()[0].id.as_deref(),
            Some("renamed-id")
        );

        let moved = temp.path().join("install-b");
        std::fs::rename(&directory, &moved).unwrap();
        let moved_entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(moved_entry.id.as_deref(), Some("renamed-id"));
        assert!(moved_entry.path.ends_with("/install-b"));

        write_manifest(temp.path(), "install-c", &manifest("renamed-id", "1"));
        assert!(
            scan_extensions_in(temp.path())
                .entries()
                .iter()
                .all(|entry| entry.status == ExtensionStatus::Duplicate)
        );
        std::fs::remove_dir_all(temp.path().join("install-c")).unwrap();
        assert_eq!(
            scan_extensions_in(temp.path()).entries()[0].status,
            ExtensionStatus::Loaded
        );

        std::fs::remove_dir_all(moved).unwrap();
        assert!(scan_extensions_in(temp.path()).extensions.is_empty());
    }
}
