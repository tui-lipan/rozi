//! Whether the public extension index would list an extension, checked before it is published.
//!
//! The index builds each entry from `extension.toml` at the root of a public GitHub repository and
//! leaves out any repository whose metadata it, or Rozi reading the index, would refuse.

use std::path::Path;

use serde::Serialize;

use super::{EXTENSION_API_VERSION, ExtensionInfo, ExtensionStatus};

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DiscoverReadiness {
    pub ready: bool,
    pub problems: Vec<String>,
}

pub(crate) fn discover_readiness(info: &ExtensionInfo) -> DiscoverReadiness {
    let mut problems = Vec::new();
    if info.status == ExtensionStatus::Invalid {
        problems
            .push("Fix the errors above: Rozi will not install an invalid extension.".to_string());
    }
    if info.api != Some(EXTENSION_API_VERSION) {
        problems.push(format!(
            "The index lists extensions with `api = {EXTENSION_API_VERSION}`."
        ));
    }
    let values = [
        info.id.as_deref(),
        info.title.as_deref(),
        info.description.as_deref(),
        info.version.as_deref(),
        info.min_rozi.as_deref(),
        info.homepage.as_deref(),
    ];
    for ((field, limit), value) in crate::extension_catalog::FIELD_LIMITS
        .into_iter()
        .zip(values)
    {
        match value {
            None if matches!(field, "id" | "title" | "description" | "version") => {
                problems.push(format!("Add `{field}` to `[extension]`."));
            }
            Some(value) if value.chars().count() > limit => problems.push(format!(
                "`{field}` is {} characters; the index takes at most {limit}.",
                value.chars().count()
            )),
            _ => {}
        }
    }
    if let Some(homepage) = info.homepage.as_deref()
        && !listable_homepage(homepage)
    {
        problems.push(
            "`homepage` must be an http or https URL with a host name and no user name."
                .to_string(),
        );
    }
    if let Some(platform) = info
        .platforms
        .iter()
        .find(|platform| !crate::extension_catalog::KNOWN_PLATFORMS.contains(&platform.as_str()))
    {
        problems.push(format!(
            "The index does not know the platform `{platform}`; use {}.",
            crate::extension_catalog::KNOWN_PLATFORMS.join(", ")
        ));
    }
    if let Some(problem) = repository_root_problem(Path::new(&info.path)) {
        problems.push(problem);
    }
    DiscoverReadiness {
        ready: problems.is_empty(),
        problems,
    }
}

fn listable_homepage(homepage: &str) -> bool {
    if !homepage.is_ascii()
        || homepage
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
    {
        return false;
    }
    let Ok(url) = url::Url::parse(homepage) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            !host.is_empty()
                && host.split('.').all(|label| {
                    !label.is_empty()
                        && !label.starts_with('-')
                        && !label.ends_with('-')
                        && label
                            .chars()
                            .all(|character| character.is_ascii_alphanumeric() || character == '-')
                })
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
