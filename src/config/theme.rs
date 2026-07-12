use std::fs;
use std::path::PathBuf;

use tui_lipan::prelude::*;

use crate::state::ThemePreset;

use super::file::config_home;
use super::schema::ThemeChoice;

/// Directory holding custom theme files. Each `*.toml` file is a theme named by its stem.
pub fn themes_dir() -> PathBuf {
    config_home().join("hyprmux/themes")
}

/// Path a custom theme named `name` would live at (whether or not it exists).
pub fn custom_theme_path(name: &str) -> PathBuf {
    themes_dir().join(format!("{name}.toml"))
}

/// Every custom theme file in [`themes_dir`], as `(name, path)`, sorted by name.
pub fn list_custom_themes() -> Vec<(String, PathBuf)> {
    let Ok(read_dir) = fs::read_dir(themes_dir()) else {
        return Vec::new();
    };
    let mut entries = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some((name, path))
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.0.cmp(&right.0));
    entries
}

/// The ordered set of selectable themes: `System`, every built-in preset not shadowed by a
/// same-named custom file, then every custom theme in [`themes_dir`].
pub fn theme_choices() -> Vec<ThemeChoice> {
    build_theme_choices(list_custom_themes())
}

fn build_theme_choices(custom: Vec<(String, PathBuf)>) -> Vec<ThemeChoice> {
    let mut choices = vec![ThemeChoice::System];
    for preset in ThemePreset::all() {
        if !custom.iter().any(|(name, _)| name == preset.id()) {
            choices.push(ThemeChoice::Builtin(preset));
        }
    }
    for (name, path) in custom {
        choices.push(ThemeChoice::Custom { name, path });
    }
    choices
}

/// Resolve a `[theme].name` to its choice. A custom file shadows the reserved `system` name
/// and any built-in preset. Returns `None` when the name matches nothing.
pub fn resolve_choice(name: &str) -> Option<ThemeChoice> {
    let path = custom_theme_path(name);
    if path.is_file() {
        return Some(ThemeChoice::Custom {
            name: name.to_string(),
            path,
        });
    }
    if name.eq_ignore_ascii_case("system") {
        return Some(ThemeChoice::System);
    }
    ThemePreset::parse(name).map(ThemeChoice::Builtin)
}

#[derive(Debug)]
pub struct ResolvedTheme {
    pub theme: Theme,
    /// The file to hot-reload while this theme is active (custom themes only).
    pub watch_path: Option<PathBuf>,
    pub warnings: Vec<String>,
}

/// Resolve a `[theme].name` to a concrete theme. `system_theme` supplies the host-derived
/// theme for the reserved `system` name; unknown names and load failures fall back to Lipan
/// with a warning.
pub fn resolve_theme(name: &str, system_theme: Option<&Theme>) -> ResolvedTheme {
    let fallback = ThemePreset::Lipan.theme();
    let mut warnings = Vec::new();
    let choice = match resolve_choice(name) {
        Some(choice) => choice,
        None => {
            warnings.push(format!("Unknown theme `{name}`; using lipan"));
            ThemeChoice::Builtin(ThemePreset::Lipan)
        }
    };
    match choice {
        ThemeChoice::System => ResolvedTheme {
            theme: system_theme.cloned().unwrap_or(fallback),
            watch_path: None,
            warnings,
        },
        ThemeChoice::Builtin(preset) => ResolvedTheme {
            theme: preset.theme(),
            watch_path: None,
            warnings,
        },
        ThemeChoice::Custom { path, .. } => {
            let theme = match load_theme_from_toml(&path, fallback.clone()) {
                Ok(theme) => theme,
                Err(err) => {
                    warnings.push(format!("Theme load failed for {}: {err}", path.display()));
                    fallback
                }
            };
            ResolvedTheme {
                theme,
                watch_path: Some(path),
                warnings,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::ThemePreset;

    #[test]
    fn theme_choices_lead_with_system_then_builtins() {
        let choices = build_theme_choices(Vec::new());
        assert_eq!(choices.first(), Some(&ThemeChoice::System));
        assert_eq!(choices.len(), 1 + ThemePreset::all().len());
        assert!(choices.contains(&ThemeChoice::Builtin(ThemePreset::Dracula)));
    }

    #[test]
    fn custom_theme_shadows_same_named_builtin() {
        let custom = vec![("dracula".to_string(), PathBuf::from("/themes/dracula.toml"))];
        let choices = build_theme_choices(custom);
        // The built-in dracula is dropped in favour of the custom file of the same name.
        assert!(!choices.contains(&ThemeChoice::Builtin(ThemePreset::Dracula)));
        assert_eq!(
            choices.last(),
            Some(&ThemeChoice::Custom {
                name: "dracula".to_string(),
                path: PathBuf::from("/themes/dracula.toml"),
            })
        );
        assert_eq!(choices.iter().filter(|c| c.label() == "dracula").count(), 1);
    }

    #[test]
    fn resolve_theme_falls_back_to_lipan_for_unknown_name() {
        let resolved = resolve_theme("definitely-not-a-real-theme-xyz", None);
        assert!(!resolved.warnings.is_empty());
        assert!(resolved.watch_path.is_none());
    }
}
