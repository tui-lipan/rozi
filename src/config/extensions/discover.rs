//! Whether the public extension index would list an extension, checked before it is published.
//!
//! The index builds each entry from `extension.toml` at the root of a public GitHub repository and
//! leaves out any repository whose metadata it, or Rozi reading the index, would refuse.

use std::path::Path;

use serde::Serialize;

use super::{EXTENSION_API_VERSION, ExtensionInfo};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscoverReadiness {
    pub ready: bool,
    pub problems: Vec<String>,
}

pub(crate) fn discover_readiness(info: &ExtensionInfo) -> DiscoverReadiness {
    let mut problems = Vec::new();
    match std::fs::read_to_string(&info.manifest_path) {
        Ok(text) => match toml::from_str::<toml::Value>(&text) {
            Ok(manifest) => check_catalog_metadata(&manifest, &mut problems),
            Err(error) => problems.push(format!("Invalid `extension.toml`: {error}")),
        },
        Err(error) => problems.push(format!("Could not read `extension.toml`: {error}")),
    }
    if let Some(problem) = repository_root_problem(Path::new(&info.path)) {
        problems.push(problem);
    }
    DiscoverReadiness {
        ready: problems.is_empty(),
        problems,
    }
}

// Read the original TOML, not ExtensionInfo's normalized install metadata. The index validates
// catalog fields and contribution array types, but does not validate contribution contents.
fn check_catalog_metadata(manifest: &toml::Value, problems: &mut Vec<String>) {
    let Some(extension) = manifest.get("extension").and_then(toml::Value::as_table) else {
        problems.push("Add `[extension]` to `extension.toml`.".to_string());
        return;
    };
    if extension.get("api").and_then(toml::Value::as_integer)
        != Some(i64::from(EXTENSION_API_VERSION))
    {
        problems.push(format!(
            "The index lists extensions with `api = {EXTENSION_API_VERSION}`."
        ));
    }
    for (field, limit) in crate::extension_catalog::FIELD_LIMITS {
        check_catalog_string(extension, field, limit, problems);
    }
    check_platforms(extension.get("platforms"), problems);
    for field in [
        "commands",
        "services",
        "agents",
        "sidebar_tabs",
        "navigation_targets",
        "suggested_keybindings",
    ] {
        if manifest.get(field).is_some_and(|value| !value.is_array()) {
            problems.push(format!("`{field}` must be an array."));
        }
    }
}

fn check_catalog_string(
    extension: &toml::Table,
    field: &str,
    limit: usize,
    problems: &mut Vec<String>,
) {
    let Some(value) = extension.get(field) else {
        if matches!(field, "id" | "title" | "description" | "version") {
            problems.push(format!("Add `{field}` to `[extension]`."));
        }
        return;
    };
    let Some(value) = value
        .as_str()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        problems.push(format!("`{field}` must be a non-empty string."));
        return;
    };
    if value.chars().count() > limit {
        problems.push(format!(
            "`{field}` is {} characters; the index takes at most {limit}.",
            value.chars().count()
        ));
    }
    if !valid_catalog_string(field, value) {
        problems.push(format!("`{field}` is not valid catalog metadata."));
    }
}

fn valid_catalog_string(field: &str, value: &str) -> bool {
    match field {
        "id" => value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'_' | b'-')
        }),
        "min_rozi" => value.parse::<semver::Version>().is_ok(),
        "homepage" => listable_homepage(value),
        _ => true,
    }
}

fn check_platforms(value: Option<&toml::Value>, problems: &mut Vec<String>) {
    let Some(value) = value else { return };
    let Some(platforms) = value.as_array() else {
        problems.push("`platforms` must be an array of strings.".to_string());
        return;
    };
    let mut seen = std::collections::HashSet::new();
    for platform in platforms {
        let Some(platform) = platform.as_str() else {
            problems.push("`platforms` must be an array of strings.".to_string());
            return;
        };
        if !crate::extension_catalog::KNOWN_PLATFORMS.contains(&platform) {
            problems.push(format!(
                "The index does not know the platform `{platform}`; use {}.",
                crate::extension_catalog::KNOWN_PLATFORMS.join(", ")
            ));
        }
        if !seen.insert(platform) {
            problems.push(format!("`platforms` contains duplicate `{platform}`."));
        }
    }
}

fn listable_homepage(homepage: &str) -> bool {
    if homepage
        .chars()
        .any(|character| character.is_whitespace() || character < '\u{20}' || character == '\u{7f}')
    {
        return false;
    }
    let Ok(url) = url::Url::parse(homepage) else {
        return false;
    };
    // Check the original hostname, before Url applies IDNA or repairs a malformed authority.
    // Unicode paths and queries are allowed; Unicode hostnames are not.
    let Some((_, rest)) = homepage.split_once("://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host = authority.split(':').next().unwrap_or_default();
    matches!(url.scheme(), "http" | "https") && !authority.contains('@') && listable_hostname(host)
}

fn listable_hostname(host: &str) -> bool {
    !host.is_empty()
        && host.split('.').all(|label| {
            !label.is_empty()
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        })
}

/// The index reads `extension.toml` only at the root of the repository.
fn repository_root_problem(directory: &Path) -> Option<String> {
    let root = directory
        .ancestors()
        .find(|ancestor| ancestor.join(".git").exists());
    match root {
        None => Some(
            "This directory is not a Git repository. Discover lists public GitHub repositories \
             with `extension.toml` at their root."
                .to_string(),
        ),
        Some(root) if root != directory => {
            // Git spells repository paths with `/` on every platform.
            let relative = directory
                .strip_prefix(root)
                .unwrap_or(directory)
                .components()
                .map(|part| part.as_os_str().to_string_lossy())
                .collect::<Vec<_>>()
                .join("/");
            Some(format!(
                "`extension.toml` is in `{relative}` of its repository; the index reads it only at the root."
            ))
        }
        Some(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn readiness(
        manifest: &str,
        layout: impl FnOnce(&Path) -> std::path::PathBuf,
    ) -> DiscoverReadiness {
        let temp = tempfile::tempdir().unwrap();
        let directory = layout(temp.path());
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(directory.join("extension.toml"), manifest).unwrap();
        discover_readiness(&crate::config::check_extension(&directory).info)
    }

    fn repository(root: &Path) -> std::path::PathBuf {
        std::fs::create_dir_all(root.join(".git")).unwrap();
        root.to_path_buf()
    }

    const LISTABLE: &str = "[extension]\nid = \"x\"\ntitle = \"X\"\ndescription = \"Does x\"\n\
        version = \"1.0.0\"\napi = 1\nhomepage = \"https://example.org/x\"\n";

    #[test]
    fn a_complete_manifest_at_a_repository_root_is_ready() {
        assert_eq!(
            readiness(LISTABLE, repository),
            DiscoverReadiness {
                ready: true,
                problems: Vec::new()
            }
        );
    }

    #[test]
    fn missing_or_oversized_metadata_is_named() {
        let manifest = format!(
            "[extension]\nid = \"x\"\ntitle = \"{}\"\nversion = \"1.0.0\"\napi = 1\n",
            "t".repeat(81)
        );
        assert_eq!(
            readiness(&manifest, repository).problems,
            [
                "`title` is 81 characters; the index takes at most 80.",
                "Add `description` to `[extension]`.",
            ]
        );
    }

    #[test]
    fn a_homepage_the_index_refuses_is_named() {
        for homepage in [
            "ftp://example.org",
            "https://user@example.org",
            "https://exa_mple.org",
        ] {
            let manifest = LISTABLE.replace("https://example.org/x", homepage);
            assert!(
                readiness(&manifest, repository)
                    .problems
                    .iter()
                    .any(|problem| problem.starts_with("`homepage`")),
                "{homepage}"
            );
        }
    }

    #[test]
    fn the_manifest_must_sit_at_the_root_of_a_repository() {
        let nested = readiness(LISTABLE, |root| {
            repository(root);
            root.join("extensions/x")
        });
        assert_eq!(
            nested.problems,
            [
                "`extension.toml` is in `extensions/x` of its repository; the index reads it only at the root."
            ]
        );
        let loose = readiness(LISTABLE, |root| root.to_path_buf());
        assert!(loose.problems[0].starts_with("This directory is not a Git repository"));
    }
}
