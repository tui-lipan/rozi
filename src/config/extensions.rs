use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use serde::Serialize;

use super::schema::{
    NamedCommand, ServiceConfig, ServiceLaunch, ServiceRestart, UserCommandAction,
};

mod authoring;
mod contributions;
mod diagnostics;
mod discovery;
mod manifest;
mod paths;
mod runtime;
mod settings;
mod validation;

pub(crate) use authoring::create_extension_scaffold;
pub use diagnostics::{
    EXTENSION_DIAGNOSTICS_SCHEMA_VERSION, ExtensionCheckDocument, ExtensionListDocument,
};
pub(crate) use diagnostics::{
    ReportKind, ReportRow, ReportSection, ReportTone, report_sections, report_text,
};
use manifest::ExtensionManifestFile;
pub(crate) use manifest::UserExtensionConfig;
use paths::{absolute_path, normalize_path};
pub use runtime::{ExtensionProvenance, GENERATION_ENV};
pub(crate) use runtime::{
    ExtensionRuntimeFingerprint, fingerprint, fingerprints_by_id, provenance_from_process,
    provenance_is_active, reconcile_generations,
};
pub(crate) use settings::merge as merge_extension_settings;
pub use settings::{ExtensionSettingValue, ExtensionSettings};
pub(crate) use validation::is_extension_scoped_id;
use validation::{
    validate_command, validate_extension_id, validate_navigation_target, validate_service,
    validate_sidebar_tab,
};

/// The generation of Rozi's complete public extension contract.
///
/// Internal Rust APIs may change independently. Manifests, injected environment, command and
/// service behavior, extension-facing control commands, and lifecycle guarantees move together.
pub const EXTENSION_API_VERSION: u32 = 1;

const RESERVED_EXTENSION_IDS: &[&str] = &["app", "command", "rozi", "user", "workspace"];
const RESERVED_EXTENSION_ENV: &[&str] = &[
    "ROZI_EXTENSION",
    "ROZI_EXTENSION_DIR",
    SETTINGS_ENV,
    runtime::GENERATION_ENV,
];

/// Where an extension process reads its merged settings, as a compact JSON object.
pub const SETTINGS_ENV: &str = "ROZI_EXTENSION_CONFIG";

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
    /// Public ids of the agent definitions this extension contributes.
    pub agents: Vec<String>,
    /// Namespaced ids of the sidebar tabs this extension contributes.
    pub sidebar_tabs: Vec<String>,
    /// Static split-aware foreground-program declarations from the manifest.
    pub navigation_targets: Vec<ExtensionNavigationTargetDiagnostic>,
    /// Settings the manifest declares, at their default values. What the user's `config.toml`
    /// overrides them to is not known here: a scan reads the extension, not the user.
    pub settings: ExtensionSettings,
    pub command_details: Vec<ExtensionCommandDiagnostic>,
    pub service_details: Vec<ExtensionServiceDiagnostic>,
    pub command_paths: BTreeMap<String, String>,
    pub service_paths: BTreeMap<String, String>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionNavigationTargetDiagnostic {
    pub name: String,
    pub programs: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionCommandDiagnostic {
    pub id: String,
    pub launch: ExtensionLaunchDiagnostic,
    /// Commands inherit the focused pane's live project directory when invoked.
    pub cwd: String,
    pub injected_env: BTreeMap<String, String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ExtensionServiceDiagnostic {
    pub id: String,
    pub launch: ExtensionLaunchDiagnostic,
    pub cwd: String,
    pub restart: String,
    /// Only Rozi-owned values are shown. Manifest environment values may contain secrets.
    pub injected_env: BTreeMap<String, String>,
    pub configured_env_keys: Vec<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum ExtensionLaunchDiagnostic {
    Direct { argv: Vec<String> },
    Shell { command: String },
    Send { text: String },
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
    agents: Vec<crate::agent_detection::AgentDefinition>,
    sidebar_tabs: Vec<super::schema::SidebarTab>,
    navigation_targets: Vec<super::schema::NavigationTargetContribution>,
    settings: ExtensionSettings,
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
        settings: &BTreeMap<String, toml::Value>,
    ) -> contributions::ExtensionContributions {
        contributions::build(self, disabled, settings)
    }
}

/// The directory extensions are installed in. Exposed so a report can name where it looked.
pub(crate) fn extensions_dir_path() -> std::path::PathBuf {
    crate::platform::paths::extensions_dir(&crate::platform::paths::PlatformEnv::from_process())
}

pub(crate) fn scan_extensions() -> ExtensionScan {
    let env = crate::platform::paths::PlatformEnv::from_process();
    scan_extensions_in(&crate::platform::paths::extensions_dir(&env))
}

pub(crate) fn validate_extension_installation_id(id: &str) -> Result<(), String> {
    let mut errors = Vec::new();
    if validate_extension_id(Some(id), &mut errors) && errors.is_empty() {
        Ok(())
    } else {
        Err(errors
            .into_iter()
            .next()
            .unwrap_or_else(|| format!("invalid extension id `{id}`")))
    }
}

pub(crate) fn scan_extensions_for_cli() -> ExtensionScan {
    let user = read_user_extension_config().unwrap_or_default();
    scan_extensions_with_user_config(&user)
}

pub(crate) fn scan_extensions_with_user_config(user: &UserExtensionConfig) -> ExtensionScan {
    let mut scan = scan_extensions();
    scan.apply_disabled(&user.disabled);
    scan
}

pub(crate) fn parse_user_extension_config(
    text: &str,
) -> Result<UserExtensionConfig, toml::de::Error> {
    super::file::parse_extensions_config(text).map(|config| UserExtensionConfig {
        disabled: config.disabled,
        settings: config.settings,
    })
}

pub(crate) fn read_user_extension_config() -> Result<UserExtensionConfig, String> {
    let path = super::config_path();
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Default::default()),
        Err(error) => {
            return Err(format!("Could not read config {}: {error}", path.display()));
        }
    };
    parse_user_extension_config(&text)
        .map_err(|error| format!("Could not parse config {}: {error}", path.display()))
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
        agents: Vec::new(),
        sidebar_tabs: Vec::new(),
        navigation_targets: Vec::new(),
        settings: ExtensionSettings::new(),
        command_details: Vec::new(),
        service_details: Vec::new(),
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
                agents: Vec::new(),
                sidebar_tabs: Vec::new(),
                navigation_targets: Vec::new(),
                settings: ExtensionSettings::new(),
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
                agents: Vec::new(),
                sidebar_tabs: Vec::new(),
                navigation_targets: Vec::new(),
                settings: ExtensionSettings::new(),
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
                agents: Vec::new(),
                sidebar_tabs: Vec::new(),
                navigation_targets: Vec::new(),
                settings: ExtensionSettings::new(),
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
    let declared_settings = settings::declared(manifest.settings, &mut info.errors);
    info.settings = declared_settings.clone();
    let mut sidebar_tabs = Vec::new();
    let mut seen_sidebar_tabs = HashSet::new();
    for raw in manifest.sidebar_tabs {
        validate_sidebar_tab(
            raw,
            &validation_id,
            &extension_dir,
            &command_env,
            &mut seen_sidebar_tabs,
            &mut info,
            &mut sidebar_tabs,
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
    let mut navigation_targets = Vec::new();
    let mut seen_navigation_targets = HashSet::new();
    for raw in manifest.navigation_targets {
        validate_navigation_target(
            raw,
            &validation_id,
            &mut seen_navigation_targets,
            &mut info,
            &mut navigation_targets,
        );
    }
    // Agent definitions are declarative data rather than a launchable process, so they need no
    // path resolution or environment - only the same validation `config.toml` entries get. An
    // invalid one invalidates the extension, matching how a bad command or service is treated.
    let mut agent_errors = Vec::new();
    let agents = crate::agent_detection::build_definitions(
        manifest.agents,
        crate::agent_detection::AgentOrigin::Extension(&validation_id),
        &[],
        &mut agent_errors,
    );
    info.errors.extend(agent_errors);
    info.agents = agents.iter().map(|agent| agent.id().to_string()).collect();

    info.command_details = commands.iter().map(command_diagnostic).collect();
    info.service_details = services.iter().map(service_diagnostic).collect();
    if !id_valid {
        info.commands.clear();
        info.services.clear();
        info.agents.clear();
        info.sidebar_tabs.clear();
        info.navigation_targets.clear();
        info.settings.clear();
        info.command_details.clear();
        info.service_details.clear();
        info.command_paths.clear();
        info.service_paths.clear();
    }

    let mut agents = agents;
    if let Some(error) = compatibility_error {
        info.status = ExtensionStatus::Incompatible;
        info.errors.insert(0, error);
        commands.clear();
        services.clear();
        agents.clear();
        sidebar_tabs.clear();
        navigation_targets.clear();
    } else if !info.errors.is_empty() {
        info.status = ExtensionStatus::Invalid;
        commands.clear();
        services.clear();
        agents.clear();
        sidebar_tabs.clear();
        navigation_targets.clear();
    } else {
        info.status = ExtensionStatus::Loaded;
        info.enabled = true;
    }
    DiscoveredExtension {
        info,
        commands,
        services,
        agents,
        sidebar_tabs,
        navigation_targets,
        settings: declared_settings,
    }
}

fn command_diagnostic(command: &NamedCommand) -> ExtensionCommandDiagnostic {
    let launches_process = !matches!(&command.action, UserCommandAction::Send(_));
    let launch = match &command.action {
        UserCommandAction::ExecDirect { argv } => {
            ExtensionLaunchDiagnostic::Direct { argv: argv.clone() }
        }
        UserCommandAction::Exec { command } => ExtensionLaunchDiagnostic::Shell {
            command: command.clone(),
        },
        UserCommandAction::Send(text) => ExtensionLaunchDiagnostic::Send { text: text.clone() },
        UserCommandAction::Run { command, .. } | UserCommandAction::Popup { command, .. } => {
            ExtensionLaunchDiagnostic::Shell {
                command: command.clone(),
            }
        }
    };
    ExtensionCommandDiagnostic {
        id: command.id.clone(),
        launch,
        cwd: if launches_process {
            "focused-pane"
        } else {
            "focused-pane-input"
        }
        .to_string(),
        injected_env: if launches_process {
            diagnostic_extension_env(&command.env)
        } else {
            BTreeMap::new()
        },
    }
}

fn service_diagnostic(service: &ServiceConfig) -> ExtensionServiceDiagnostic {
    let launch = match &service.launch {
        ServiceLaunch::Direct(argv) => ExtensionLaunchDiagnostic::Direct { argv: argv.clone() },
        ServiceLaunch::Shell(command) => ExtensionLaunchDiagnostic::Shell {
            command: command.clone(),
        },
    };
    ExtensionServiceDiagnostic {
        id: service.name.clone(),
        launch,
        cwd: service.cwd.clone().unwrap_or_else(|| ".".to_string()),
        restart: match service.restart {
            ServiceRestart::Always => "always",
            ServiceRestart::OnFailure => "on-failure",
            ServiceRestart::Never => "never",
        }
        .to_string(),
        injected_env: diagnostic_extension_env(
            &service
                .env
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>(),
        ),
        configured_env_keys: service
            .env
            .keys()
            .filter(|key| !RESERVED_EXTENSION_ENV.contains(&key.as_str()))
            .cloned()
            .collect(),
    }
}

fn diagnostic_extension_env(env: &[(String, String)]) -> BTreeMap<String, String> {
    let mut values: BTreeMap<_, _> = env
        .iter()
        .filter(|(key, _)| RESERVED_EXTENSION_ENV.contains(&key.as_str()))
        .cloned()
        .collect();
    values.insert(
        runtime::GENERATION_ENV.to_string(),
        "<assigned-at-load>".to_string(),
    );
    values
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
    fn navigation_targets_are_validated_and_contributed_as_static_policy() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "vim-rozi",
            &format!(
                "{}[[navigation_targets]]\nname = \"vim\"\nprograms = [\"vim\", \"nvim\", \"NVIM\"]\n",
                manifest("vim-rozi", "1")
            ),
        );

        let scan = scan_extensions_in(temp.path());
        let entries = scan.entries();
        assert_eq!(entries[0].status, ExtensionStatus::Loaded);
        assert_eq!(entries[0].navigation_targets.len(), 1);
        assert_eq!(entries[0].navigation_targets[0].name, "vim");
        assert_eq!(entries[0].navigation_targets[0].programs, ["vim", "nvim"]);

        let contributions = scan.into_contributions(&[], &Default::default());
        assert_eq!(contributions.navigation_targets.len(), 1);
        assert_eq!(contributions.navigation_targets[0].extension_id, "vim-rozi");
        assert_eq!(contributions.navigation_targets[0].name, "vim");
    }

    #[test]
    fn an_invalid_navigation_target_invalidates_the_extension_atomically() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "bad-navigation",
            &format!(
                "{}[[commands]]\nid = \"run\"\nsend = \"run\"\n\
                 [[navigation_targets]]\nname = \"vim\"\nprograms = [\"bin/vim\"]\n",
                manifest("bad-navigation", "1")
            ),
        );

        let scan = scan_extensions_in(temp.path());
        let entries = scan.entries();
        let entry = &entries[0];
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(
            entry
                .errors
                .iter()
                .any(|error| error.contains("must be an executable basename"))
        );
        let contributions = scan.into_contributions(&[], &Default::default());
        assert!(contributions.commands.is_empty());
        assert!(contributions.navigation_targets.is_empty());
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

        let contributions = scan.into_contributions(&[], &Default::default());
        let (commands, services, active, runtime, warnings) = (
            contributions.commands,
            contributions.services,
            contributions.active_ids,
            contributions.runtime,
            contributions.warnings,
        );
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
            Path::new(&command[0]).ends_with(Path::new("rozi-git-tools/bin/branches")),
            "{command:?}"
        );
        assert_eq!(services[0].name, "git-tools.watch");
        assert_eq!(
            services[0].cwd.as_deref(),
            Some(directory.display().to_string().as_str())
        );
        assert!(matches!(
            &services[0].launch,
            ServiceLaunch::Direct(argv)
                if Path::new(&argv[0]).ends_with(Path::new("rozi-git-tools/bin/watch"))
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
            .find(|entry| Path::new(&entry.path).ends_with("missing"))
            .unwrap();
        assert_eq!(malformed.status, ExtensionStatus::Invalid);
        assert!(malformed.errors[0].contains("invalid extension.toml"));
        let contributions = scan.into_contributions(&[], &Default::default());
        let (commands, services, active) = (
            contributions.commands,
            contributions.services,
            contributions.active_ids,
        );
        assert!(commands.is_empty());
        assert!(services.is_empty());
        assert!(active.is_empty());
    }

    #[test]
    fn extension_agents_are_namespaced_contributions() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "rozi-mytool",
            &format!(
                "{}[[agents]]\nid = \"mytool\"\nlabel = \"My Tool\"\n\
                 match = {{ names = [\"mytool\"] }}\n\
                 [[agents.states]]\nstate = \"blocked\"\n\
                 screen = {{ any_of = [\"approve? (a/d)\"] }}\n",
                manifest("mytool", "1")
            ),
        );
        let scan = scan_extensions_in(temp.path());
        assert_eq!(scan.entries()[0].status, ExtensionStatus::Loaded);
        assert_eq!(scan.entries()[0].agents, ["mytool.mytool"]);

        let contributions = scan.into_contributions(&[], &Default::default());
        assert_eq!(contributions.agents.len(), 1);
        assert_eq!(contributions.agents[0].id(), "mytool.mytool");
        assert_eq!(contributions.agents[0].label(), "My Tool");
        let catalog = crate::agent_detection::AgentCatalog::with_definitions(contributions.agents);
        assert_eq!(
            catalog.by_name("mytool").map(|agent| agent.id()),
            Some("mytool.mytool")
        );
    }

    #[test]
    fn an_invalid_agent_definition_invalidates_the_whole_extension() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "rozi-broken",
            &format!(
                "{}[[commands]]\nid = \"ok\"\nsend = \"hi\"\n\
                 [[agents]]\nid = \"Bad Id\"\nmatch = {{ names = [\"x\"] }}\n",
                manifest("broken", "1")
            ),
        );
        let scan = scan_extensions_in(temp.path());
        let entry = &scan.entries()[0];
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(
            entry
                .errors
                .iter()
                .any(|error| error.contains("invalid id")),
            "{:?}",
            entry.errors
        );
        let contributions = scan.into_contributions(&[], &Default::default());
        assert!(
            contributions.commands.is_empty(),
            "one bad agent invalidates the extension atomically, commands included"
        );
        assert!(contributions.agents.is_empty());
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
        let one = temp.path().join("one").display().to_string();
        let two = temp.path().join("two").display().to_string();
        assert!(scan.entries().iter().all(|entry| {
            entry.status == ExtensionStatus::Duplicate
                && entry.errors[0].contains(&one)
                && entry.errors[0].contains(&two)
        }));
        let contributions = scan.into_contributions(&[], &Default::default());
        let (commands, services, active) = (
            contributions.commands,
            contributions.services,
            contributions.active_ids,
        );
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
        let contributions = scan.into_contributions(&["docker".to_string()], &Default::default());
        assert!(contributions.commands.is_empty());
        assert!(contributions.services.is_empty());
        assert!(contributions.agents.is_empty());
        assert!(contributions.active_ids.is_empty());
        assert!(contributions.warnings.is_empty());
    }

    #[test]
    fn one_bad_extension_does_not_hide_sorted_neighbors() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(temp.path(), "z-good", &manifest("z-good", "1"));
        write_manifest(temp.path(), "a-broken", "not toml");
        write_manifest(temp.path(), "m-good", &manifest("m-good", "1"));
        let entries = scan_extensions_in(temp.path()).entries();
        assert!(Path::new(&entries[0].path).ends_with("a-broken"));
        assert!(Path::new(&entries[1].path).ends_with("m-good"));
        assert!(Path::new(&entries[2].path).ends_with("z-good"));
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

    #[test]
    fn missing_path_program_is_reported_before_launch() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "missing-program",
            &format!(
                "{}[[commands]]\nid = \"open\"\nexec = [\"rozi-program-that-does-not-exist-42\"]\n",
                manifest("missing-program", "1")
            ),
        );
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(entry.errors.iter().any(|error| {
            error.contains("executable `rozi-program-that-does-not-exist-42` was not found on PATH")
        }));
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
    fn broken_symlink_is_a_diagnostic_candidate() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().unwrap();
        symlink(
            temp.path().join("missing-target"),
            temp.path().join("broken"),
        )
        .unwrap();

        let entries = scan_extensions_in(temp.path()).entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ExtensionStatus::Invalid);
        assert!(
            entries[0]
                .errors
                .iter()
                .any(|error| error.contains("could not read extension.toml"))
        );
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn non_utf8_installation_is_a_diagnostic_candidate() {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        let temp = tempfile::tempdir().unwrap();
        let non_utf8 = temp
            .path()
            .join(OsString::from_vec(b"non-utf8-\xff".to_vec()));
        std::fs::create_dir_all(&non_utf8).unwrap();
        std::fs::write(non_utf8.join("extension.toml"), manifest("non-utf8", "1")).unwrap();

        let entries = scan_extensions_in(temp.path()).entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].status, ExtensionStatus::Invalid);
        assert!(
            entries
                .iter()
                .flat_map(|entry| &entry.errors)
                .any(|error| error.contains("not valid UTF-8"))
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
        for directory in [
            "git-tools",
            "activity-dashboard",
            "pr-dashboard",
            "docker",
            "ssh-tools",
            "agent-activity",
        ] {
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
        assert_eq!(
            schema["properties"]["navigation_targets"]["items"]["required"],
            serde_json::json!(["name", "programs"])
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

        let contributions = scan_extensions_in(temp.path())
            .into_contributions(&["matrix".to_string()], &Default::default());
        assert!(contributions.commands.is_empty());
        assert!(contributions.active_ids.is_empty());

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
        assert!(Path::new(&moved_entry.path).ends_with("install-b"));

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

    #[test]
    fn sidebar_tabs_are_namespaced_and_contributed_like_commands() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "git-tools",
            &format!(
                "{}[[sidebar_tabs]]\nname = \"agents\"\nlabel = \"Agents\"\nentries = [\n\
                 {{ label = \"rozi\", group = \"claude\", run = \"claude\" }},\n\
                 ]\n",
                manifest("git-tools", "1")
            ),
        );

        let scan = scan_extensions_in(temp.path());
        let entries = scan.entries();
        assert_eq!(entries[0].status, ExtensionStatus::Loaded);
        assert_eq!(entries[0].sidebar_tabs, ["git-tools.agents"]);

        let contributions = scan.into_contributions(&[], &Default::default());
        assert!(
            contributions.warnings.is_empty(),
            "{:?}",
            contributions.warnings
        );
        assert_eq!(contributions.sidebar_tabs.len(), 1);
        assert_eq!(
            contributions.sidebar_tabs[0].id(),
            super::super::schema::SidebarTabId::new("git-tools.agents")
        );
        assert!(contributions.installed_ids.contains("git-tools"));
    }

    /// A tab that cannot be built takes the extension down with it, the same as a malformed command
    /// or service — an extension is either wholly trustworthy or not loaded.
    #[test]
    fn a_malformed_sidebar_tab_invalidates_the_extension() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "broken",
            &format!(
                "{}[[sidebar_tabs]]\nname = \"tasks\"\nlabel = \"Tasks\"\n",
                manifest("broken", "1")
            ),
        );
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(entry.sidebar_tabs.is_empty());
        assert!(
            entry
                .errors
                .iter()
                .any(|error| error.contains("exactly one of `entries` or `command`")),
            "{:?}",
            entry.errors
        );
    }

    /// Settings travel to the process as one JSON environment variable, and because that variable is
    /// part of the command and service environment, changing a setting is a process-facing change:
    /// the fingerprint moves and the generation rotates, restarting whatever was reading the old
    /// value. A metadata edit still must not.
    #[test]
    fn settings_reach_the_process_environment_and_move_the_runtime_fingerprint() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "tasks",
            &format!(
                "{}[[commands]]\nid = \"run\"\nsend = \"run\"\n\
                 [[services]]\nname = \"watch\"\nshell = \"watch\"\n\
                 [settings]\nrunner = \"auto\"\nrows = 50\n",
                manifest("tasks", "1")
            ),
        );
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Loaded);
        assert_eq!(
            entry.settings["runner"],
            ExtensionSettingValue::String("auto".to_string())
        );

        let defaults = scan_extensions_in(temp.path()).into_contributions(&[], &Default::default());
        let value = |contributions: &contributions::ExtensionContributions| {
            contributions.commands[0]
                .env
                .iter()
                .find(|(key, _)| key == SETTINGS_ENV)
                .map(|(_, value)| value.clone())
                .expect("settings reach the command environment")
        };
        assert_eq!(value(&defaults), r#"{"rows":50,"runner":"auto"}"#);
        assert_eq!(
            defaults.services[0].env[SETTINGS_ENV],
            r#"{"rows":50,"runner":"auto"}"#
        );

        let user: BTreeMap<String, toml::Value> = [(
            "tasks".to_string(),
            toml::from_str::<toml::Value>("runner = \"just\"").unwrap(),
        )]
        .into_iter()
        .collect();
        let overridden = scan_extensions_in(temp.path()).into_contributions(&[], &user);
        assert!(overridden.warnings.is_empty(), "{:?}", overridden.warnings);
        assert_eq!(value(&overridden), r#"{"rows":50,"runner":"just"}"#);
        assert_ne!(
            defaults.runtime["tasks"], overridden.runtime["tasks"],
            "a changed setting is a process-facing change"
        );
    }

    #[test]
    fn settings_report_unknown_keys_wrong_types_and_orphan_tables() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "tasks",
            &format!("{}[settings]\nrows = 50\n", manifest("tasks", "1")),
        );
        let user: BTreeMap<String, toml::Value> = [
            (
                "tasks".to_string(),
                toml::from_str::<toml::Value>("rows = \"many\"\nnope = 1").unwrap(),
            ),
            (
                "not-installed".to_string(),
                toml::from_str::<toml::Value>("anything = 1").unwrap(),
            ),
        ]
        .into_iter()
        .collect();
        let warnings = scan_extensions_in(temp.path())
            .into_contributions(&[], &user)
            .warnings;
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("no setting `nope`")));
        assert!(warnings.iter().any(|w| w.contains("is string")));
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("no installed extension"))
        );
    }

    #[test]
    fn user_extension_config_parser_keeps_disabled_ids_and_raw_setting_tables() {
        let parsed = parse_user_extension_config(
            "[extensions]\n\
             disabled = [\"git-tools\"]\n\
             [extensions.tasks]\n\
             runner = \"just\"\n\
             rows = 50\n",
        )
        .expect("extension config parses");
        assert_eq!(parsed.disabled, ["git-tools"]);
        assert_eq!(parsed.settings["tasks"]["runner"].as_str(), Some("just"));
        assert_eq!(parsed.settings["tasks"]["rows"].as_integer(), Some(50));
        assert!(
            parse_user_extension_config(
                "unknown_top_level = true\n[extensions]\ndisabled = [\"tasks\"]\n"
            )
            .is_err(),
            "manager parsing must reject the same document as the runtime loader"
        );
    }

    /// A setting Rozi cannot carry is the extension's bug, not the user's, so it fails at load the
    /// way a malformed command does rather than disappearing quietly.
    #[test]
    fn an_uncarriable_setting_invalidates_the_extension() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "tasks",
            &format!("{}[settings]\nratio = 0.5\n", manifest("tasks", "1")),
        );
        let entry = scan_extensions_in(temp.path()).entries().remove(0);
        assert_eq!(entry.status, ExtensionStatus::Invalid);
        assert!(
            entry.errors.iter().any(|error| error.contains("`ratio`")),
            "{:?}",
            entry.errors
        );
    }

    /// The frozen shape of extension API 1.
    ///
    /// Every key here is a promise (`docs/extensions.md#stability`). Adding one is compatible and
    /// means adding it to this list too; removing or renaming one is not, and needs `api = 2`. The
    /// list is spelled out rather than derived so the diff, not the reader, catches a change.
    #[test]
    fn the_api_1_manifest_surface_is_frozen() {
        let schema: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(
                Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/extension.schema.json"),
            )
            .unwrap(),
        )
        .unwrap();
        let keys = |value: &serde_json::Value| {
            let mut keys: Vec<String> = value
                .as_object()
                .expect("object schema")
                .keys()
                .cloned()
                .collect();
            keys.sort();
            keys
        };
        let properties = &schema["properties"];
        assert_eq!(
            keys(properties),
            [
                "agents",
                "commands",
                "extension",
                "navigation_targets",
                "services",
                "settings",
                "sidebar_tabs"
            ]
        );
        assert_eq!(
            keys(&properties["extension"]["properties"]),
            ["api", "description", "id", "title", "version"]
        );
        assert_eq!(
            keys(&properties["commands"]["items"]["properties"]),
            ["exec", "id", "key", "label", "send", "shell"]
        );
        assert_eq!(
            keys(&properties["services"]["items"]["properties"]),
            ["cwd", "env", "exec", "name", "restart", "shell"]
        );
        assert_eq!(
            keys(&properties["navigation_targets"]["items"]["properties"]),
            ["name", "programs"]
        );
        assert_eq!(
            keys(&properties["sidebar_tabs"]["items"]["properties"]),
            [
                "command",
                "entries",
                "group_prefix",
                "interval",
                "label",
                "name",
                "on_click"
            ]
        );
        assert_eq!(
            keys(
                &properties["sidebar_tabs"]["items"]["properties"]["entries"]["items"]["properties"]
            ),
            ["group", "keep_open", "label", "popup", "run", "send"]
        );
        // The environment every extension process is promised, and the API and diagnostics
        // generations that describe the whole contract.
        assert_eq!(
            RESERVED_EXTENSION_ENV,
            [
                "ROZI_EXTENSION",
                "ROZI_EXTENSION_DIR",
                "ROZI_EXTENSION_CONFIG",
                "ROZI_EXTENSION_GENERATION"
            ]
        );
        assert_eq!(EXTENSION_API_VERSION, 1);
        assert_eq!(EXTENSION_DIAGNOSTICS_SCHEMA_VERSION, 1);
    }

    /// A contributed tab is neither a command nor a service, so nothing else would tell it where it
    /// lives or what the user configured. `{extension_dir}` is substituted into its command lines,
    /// and the same environment a command receives rides along for the process behind them.
    #[test]
    fn a_sidebar_tab_is_told_where_it_lives_and_what_the_user_configured() {
        let temp = tempfile::tempdir().unwrap();
        let directory = write_manifest(
            temp.path(),
            "tasks",
            &format!(
                "{}[settings]\nrunner = \"just\"\n\
                 [[sidebar_tabs]]\nname = \"list\"\nlabel = \"Tasks\"\n\
                 command = \"python {{extension_dir}}/bin/tasks.py list\"\n\
                 on_click = {{ run = \"python {{extension_dir}}/bin/tasks.py run\" }}\n",
                manifest("tasks", "1")
            ),
        );
        let contributions =
            scan_extensions_in(temp.path()).into_contributions(&[], &Default::default());
        let tab = &contributions.sidebar_tabs[0];
        let crate::config::SidebarTab::Command {
            command, on_click, ..
        } = tab
        else {
            panic!("command tab");
        };
        let expected = directory.display().to_string();
        assert_eq!(*command, format!("python {expected}/bin/tasks.py list"));
        assert!(
            on_click
                .as_ref()
                .is_some_and(|action| action.target().contains(&expected)),
            "on_click keeps its resolved path"
        );
        let env: std::collections::HashMap<_, _> = tab.env().iter().cloned().collect();
        assert_eq!(env["ROZI_EXTENSION"], "tasks");
        assert_eq!(env["ROZI_EXTENSION_DIR"], expected);
        assert_eq!(env[SETTINGS_ENV], r#"{"runner":"just"}"#);
    }

    /// Disabling an extension withdraws its tab but keeps the extension in `installed_ids`, which is
    /// what lets a sidebar placement naming that tab survive until the extension is really gone.
    #[test]
    fn a_disabled_extension_contributes_nothing_but_stays_installed() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "git-tools",
            &format!(
                "{}[[sidebar_tabs]]\nname = \"agents\"\nlabel = \"Agents\"\n\
                 entries = [{{ label = \"rozi\", run = \"claude\" }}]\n",
                manifest("git-tools", "1")
            ),
        );
        let contributions = scan_extensions_in(temp.path())
            .into_contributions(&["git-tools".to_string()], &Default::default());
        assert!(contributions.sidebar_tabs.is_empty());
        assert!(!contributions.active_ids.contains("git-tools"));
        assert!(contributions.installed_ids.contains("git-tools"));
    }
}
