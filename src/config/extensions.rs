use std::collections::HashSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::commands::valid_command_segment;
use super::file::{NamedCommandFileConfig, ServiceFileConfig, UserCommandTableSpec};
use super::input::parse_user_command_action;
use super::schema::NamedCommand;

#[derive(Clone, Debug)]
pub(crate) struct DiscoveredExtension {
    pub(crate) id: String,
    pub(crate) title: Option<String>,
    pub(crate) directory: PathBuf,
    commands: Vec<NamedCommandFileConfig>,
    services: Vec<ServiceFileConfig>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ExtensionInfo {
    pub id: String,
    pub title: Option<String>,
    pub version: Option<String>,
    pub commands: usize,
    pub services: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Default)]
pub(crate) struct ExtensionScan {
    pub(crate) extensions: Vec<DiscoveredExtension>,
    pub(crate) entries: Vec<ExtensionInfo>,
    pub(crate) warnings: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionManifestFile {
    extension: ExtensionMetadataFile,
    #[serde(default)]
    commands: Vec<NamedCommandFileConfig>,
    #[serde(default)]
    services: Vec<ServiceFileConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExtensionMetadataFile {
    title: Option<String>,
    description: Option<String>,
    version: Option<String>,
}

pub(crate) fn scan_extensions() -> ExtensionScan {
    let env = crate::platform::paths::PlatformEnv::from_process();
    scan_extensions_in(&crate::platform::paths::extensions_dir(&env))
}

pub(crate) fn scan_extensions_in(root: &Path) -> ExtensionScan {
    let mut scan = ExtensionScan::default();
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return scan,
        Err(error) => {
            scan.warnings.push(format!(
                "Extensions directory read failed for {}: {error}",
                root.display()
            ));
            return scan;
        }
    };
    let mut directories: Vec<_> = entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .collect();
    directories.sort_by_key(|entry| entry.file_name());

    for entry in directories {
        let id = entry.file_name().to_string_lossy().to_string();
        if !valid_command_segment(&id) {
            let error = "directory name must match [a-z0-9_-]+".to_string();
            scan.warnings
                .push(format!("extension `{id}` {error}; skipped"));
            scan.entries.push(ExtensionInfo {
                id,
                title: None,
                version: None,
                commands: 0,
                services: 0,
                error: Some(error),
            });
            continue;
        }
        let directory = std::fs::canonicalize(entry.path()).unwrap_or_else(|_| entry.path());
        let manifest_path = directory.join("extension.toml");
        let manifest_text = match std::fs::read_to_string(&manifest_path) {
            Ok(text) => text,
            Err(error) => {
                let message = format!("could not read extension.toml: {error}");
                scan.warnings
                    .push(format!("extension `{id}` {message}; skipped"));
                scan.entries.push(ExtensionInfo {
                    id,
                    title: None,
                    version: None,
                    commands: 0,
                    services: 0,
                    error: Some(message),
                });
                continue;
            }
        };
        let manifest: ExtensionManifestFile = match toml::from_str(&manifest_text) {
            Ok(manifest) => manifest,
            Err(error) => {
                let message = format!("invalid extension.toml: {error}");
                scan.warnings
                    .push(format!("extension `{id}` {message}; skipped"));
                scan.entries.push(ExtensionInfo {
                    id,
                    title: None,
                    version: None,
                    commands: 0,
                    services: 0,
                    error: Some(message),
                });
                continue;
            }
        };
        let title = clean_optional(manifest.extension.title);
        let version = clean_optional(manifest.extension.version);
        let _description = clean_optional(manifest.extension.description);
        scan.entries.push(ExtensionInfo {
            id: id.clone(),
            title: title.clone(),
            version,
            commands: manifest.commands.len(),
            services: manifest.services.len(),
            error: None,
        });
        scan.extensions.push(DiscoveredExtension {
            id,
            title,
            directory,
            commands: manifest.commands,
            services: manifest.services,
        });
    }
    scan
}

pub(crate) fn build_extension_contributions(
    extensions: Vec<DiscoveredExtension>,
    disabled: &[String],
    warnings: &mut Vec<String>,
) -> (Vec<NamedCommand>, Vec<ServiceFileConfig>) {
    let disabled: HashSet<_> = disabled.iter().map(|id| id.trim()).collect();
    let mut commands = Vec::new();
    let mut services = Vec::new();
    let mut seen_commands = HashSet::new();

    for extension in extensions {
        if disabled.contains(extension.id.as_str()) {
            continue;
        }
        let category = extension
            .title
            .clone()
            .unwrap_or_else(|| extension.id.clone());
        let extension_dir = absolute_path(&extension.directory);
        let extension_dir_text = extension_dir.display().to_string();
        let env = vec![
            ("ROZI_EXTENSION".to_string(), extension.id.clone()),
            ("ROZI_EXTENSION_DIR".to_string(), extension_dir_text.clone()),
        ];

        for raw in extension.commands {
            let Some(local_id) = raw
                .id
                .as_deref()
                .map(str::trim)
                .filter(|id| !id.is_empty())
                .map(str::to_string)
            else {
                warnings.push(format!(
                    "extension `{}` command missing required field `id`; skipped",
                    extension.id
                ));
                continue;
            };
            if !valid_command_segment(&local_id) {
                warnings.push(format!(
                    "extension `{}` command id `{local_id}` must match [a-z0-9_-]+; skipped",
                    extension.id
                ));
                continue;
            }
            let id = format!("{}.{}", extension.id, local_id);
            if !seen_commands.insert(id.clone()) {
                warnings.push(format!("duplicate extension command id `{id}`; skipped"));
                continue;
            }
            let label = clean_optional(raw.label);
            let table = UserCommandTableSpec {
                label: None,
                run: absolutize_command(raw.run, &extension_dir),
                send: raw.send,
                popup: absolutize_command(raw.popup, &extension_dir),
                exec: absolutize_command(raw.exec, &extension_dir),
                keep_open: raw.keep_open,
            };
            let Some(action) =
                parse_user_command_action(table, &format!("Extension command `{id}`"), warnings)
            else {
                continue;
            };
            commands.push(NamedCommand {
                id,
                label,
                action,
                category: category.clone(),
                env: env.clone(),
            });
        }

        for mut service in extension.services {
            service.name = service
                .name
                .as_deref()
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(|name| format!("{}.{}", extension.id, name));
            service.run = absolutize_command(service.run, &extension_dir);
            service.cwd = match clean_optional(service.cwd) {
                Some(cwd) => Some(
                    resolve_relative_path(&extension_dir, &cwd)
                        .display()
                        .to_string(),
                ),
                None => Some(extension_dir_text.clone()),
            };
            service
                .env
                .insert("ROZI_EXTENSION".to_string(), extension.id.clone());
            service
                .env
                .insert("ROZI_EXTENSION_DIR".to_string(), extension_dir_text.clone());
            services.push(service);
        }
    }

    (commands, services)
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn resolve_relative_path(base: &Path, value: &str) -> PathBuf {
    let path = Path::new(value);
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

fn absolutize_command(value: Option<String>, base: &Path) -> Option<String> {
    let value = clean_optional(value)?;
    let split = value.find(char::is_whitespace).unwrap_or(value.len());
    let (program, rest) = value.split_at(split);
    if !(program.starts_with("./") || program.starts_with("../")) {
        return Some(value);
    }
    let absolute = resolve_relative_path(base, program);
    let rendered = absolute.display().to_string();
    let rendered = if rendered.chars().any(char::is_whitespace) {
        format!("\"{}\"", rendered.replace('"', "\\\""))
    } else {
        rendered
    };
    Some(format!("{rendered}{rest}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_manifest(root: &Path, id: &str, text: &str) {
        let dir = root.join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("extension.toml"), text).unwrap();
    }

    #[test]
    fn scans_valid_and_invalid_extensions_in_name_order() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "git-tools",
            "[extension]\ntitle = \"Git tools\"\nversion = \"0.1.0\"\n[[commands]]\nid = \"branches\"\nexec = \"./bin/branches\"\n[[services]]\nname = \"watch\"\nrun = \"./bin/watch\"\n",
        );
        write_manifest(temp.path(), "Bad", "[extension]\n");
        write_manifest(temp.path(), "broken", "not toml");

        let scan = scan_extensions_in(temp.path());
        assert_eq!(scan.extensions.len(), 1);
        assert_eq!(scan.entries.len(), 3);
        assert_eq!(scan.entries[0].id, "Bad");
        assert_eq!(scan.entries[1].id, "broken");
        assert_eq!(scan.entries[2].id, "git-tools");
        assert_eq!(scan.entries[2].commands, 1);
        assert_eq!(scan.entries[2].services, 1);
    }

    #[test]
    fn contributions_namespace_ids_paths_env_and_default_service_cwd() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "git-tools",
            "[extension]\ntitle = \"Git tools\"\n[[commands]]\nid = \"branches\"\nexec = \"./bin/branches --all\"\n[[services]]\nname = \"watch\"\nrun = \"./bin/watch\"\n",
        );
        let scan = scan_extensions_in(temp.path());
        let mut warnings = scan.warnings;
        let (commands, services) =
            build_extension_contributions(scan.extensions, &[], &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(commands[0].id, "git-tools.branches");
        assert_eq!(commands[0].category, "Git tools");
        assert!(
            commands[0]
                .env
                .iter()
                .any(|(key, value)| key == "ROZI_EXTENSION" && value == "git-tools")
        );
        let command = match &commands[0].action {
            super::super::schema::UserCommandAction::Exec { command } => command,
            other => panic!("unexpected action: {other:?}"),
        };
        assert!(
            command.contains("/git-tools/bin/branches --all"),
            "{command}"
        );
        assert_eq!(services[0].name.as_deref(), Some("git-tools.watch"));
        assert_eq!(
            services[0].cwd.as_deref(),
            Some(scan_path(temp.path(), "git-tools").as_str())
        );
        assert!(
            services[0]
                .run
                .as_deref()
                .is_some_and(|run| run.contains("/git-tools/bin/watch"))
        );
    }

    #[test]
    fn disabled_extensions_contribute_nothing() {
        let temp = tempfile::tempdir().unwrap();
        write_manifest(
            temp.path(),
            "docker",
            "[extension]\n[[commands]]\nid = \"open\"\nrun = \"docker ps\"\n",
        );
        let scan = scan_extensions_in(temp.path());
        let (commands, services) = build_extension_contributions(
            scan.extensions,
            &["docker".to_string()],
            &mut Vec::new(),
        );
        assert!(commands.is_empty());
        assert!(services.is_empty());
    }

    fn scan_path(root: &Path, id: &str) -> String {
        std::fs::canonicalize(root.join(id))
            .unwrap()
            .display()
            .to_string()
    }
}
