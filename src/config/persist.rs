use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use super::file::{config_home, config_path, note_config_text};
use super::schema::ProfileEntry;

/// Writes an updated config text, creating the config directory when needed, and records the
/// text as last-seen so the live-reload watcher does not treat our own write as an edit.
fn write_config_text(path: &Path, updated: String) -> std::result::Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent).map_err(|err| {
            format!(
                "Could not create config directory {}: {err}",
                parent.display()
            )
        })?;
    }
    fs::write(path, &updated)
        .map_err(|err| format!("Could not write config {}: {err}", path.display()))?;
    note_config_text(Some(updated));
    Ok(())
}

pub fn persist_theme_name(name: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_theme_name(&text, name);
    write_config_text(&path, updated)?;
    Ok(path)
}

fn upsert_theme_name(text: &str, name: &str) -> String {
    let mut output = String::new();
    let mut in_theme = false;
    let mut saw_theme = false;
    let mut wrote_name = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_theme && !wrote_name {
                output.push_str(&format!("name = \"{name}\"\n"));
                wrote_name = true;
            }
            in_theme = trimmed == "[theme]";
            saw_theme |= in_theme;
        }

        if in_theme
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "name" | "preset" | "path"))
        {
            if !wrote_name {
                output.push_str(&format!("name = \"{name}\"\n"));
                wrote_name = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_theme && !wrote_name {
        output.push_str(&format!("name = \"{name}\"\n"));
    } else if !saw_theme {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[theme]\n");
        output.push_str(&format!("name = \"{name}\"\n"));
    }

    output
}

pub fn persist_pane_flag(key: &str, value: bool) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_bool_in_section(&text, "pane", key, value);
    write_config_text(&path, updated)?;
    Ok(path)
}

/// Persist the compact CSS-style vertical/horizontal pane padding form.
pub fn persist_pane_padding(
    vertical: u16,
    horizontal: u16,
) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };
    let updated = upsert_pane_padding(&text, vertical, horizontal);
    write_config_text(&path, updated)?;
    Ok(path)
}

/// Serialize the Appearance editor's explicit vertical/horizontal form. Kept separate from I/O
/// so all accepted source forms are covered by a deterministic persistence test.
fn upsert_pane_padding(text: &str, vertical: u16, horizontal: u16) -> String {
    upsert_value_in_section(
        text,
        "pane",
        "padding",
        &format!("[{vertical}, {horizontal}]"),
    )
}

pub fn persist_animation_flag(key: &str, value: bool) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_bool_in_section(&text, "animations", key, value);
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn persist_pane_string(key: &str, value: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_value_in_section(&text, "pane", key, &format!("\"{value}\""));
    write_config_text(&path, updated)?;
    Ok(path)
}

fn upsert_bool_in_section(text: &str, section: &str, key: &str, value: bool) -> String {
    upsert_value_in_section(text, section, key, if value { "true" } else { "false" })
}

/// Insert or replace `key = <line_value>` inside `[section]`, creating the section at the end
/// of the file when it does not exist yet. `line_value` is written verbatim (already quoted for
/// strings, bare for bools/numbers).
fn upsert_value_in_section(text: &str, section: &str, key: &str, line_value: &str) -> String {
    let section_header = format!("[{section}]");
    let mut output = String::new();
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote_key = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_section && !wrote_key {
                output.push_str(&format!("{key} = {line_value}\n"));
                wrote_key = true;
            }
            in_section = trimmed == section_header;
            saw_section |= in_section;
        }

        if in_section
            && trimmed
                .split_once('=')
                .is_some_and(|(candidate, _)| candidate.trim() == key)
        {
            if !wrote_key {
                output.push_str(&format!("{key} = {line_value}\n"));
                wrote_key = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_section && !wrote_key {
        output.push_str(&format!("{key} = {line_value}\n"));
    } else if !saw_section {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str(&section_header);
        output.push('\n');
        output.push_str(&format!("{key} = {line_value}\n"));
    }

    output
}

#[cfg(test)]
mod padding_persistence_tests {
    use super::*;

    #[test]
    fn pane_padding_upsert_replaces_every_source_form_and_creates_pane_section() {
        for source in [
            "[pane]\npadding = 2\n",
            "[pane]\npadding = [1, 2]\n",
            "[pane]\npadding = [1, 2, 3, 4]\n",
        ] {
            assert_eq!(
                upsert_pane_padding(source, 3, 4),
                "[pane]\npadding = [3, 4]\n"
            );
        }
        assert_eq!(
            upsert_pane_padding("[theme]\nname = \"dark\"\n", 3, 4),
            "[theme]\nname = \"dark\"\n\n[pane]\npadding = [3, 4]\n"
        );
    }
}

pub fn profiles_dir() -> PathBuf {
    config_home().join("profiles")
}

pub fn profile_path_for_name(name: &str) -> PathBuf {
    profiles_dir().join(format!("{name}.toml"))
}

pub fn list_profiles() -> Vec<ProfileEntry> {
    let dir = profiles_dir();
    let Ok(read_dir) = fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut entries = read_dir
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "toml"))
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_stem()?.to_string_lossy().into_owned();
            Some(ProfileEntry { name, path })
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| left.name.cmp(&right.name));
    entries
}

pub fn persist_default_profile(name: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_default_profile(&text, name);
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn delete_profile_file(path: &Path) -> std::result::Result<(), String> {
    match fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Err(format!("Not a profile file: {}", path.display()));
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(err) => {
            return Err(format!(
                "Could not inspect profile {}: {err}",
                path.display()
            ));
        }
        Ok(_) => {}
    }

    fs::remove_file(path)
        .map_err(|err| format!("Could not delete profile {}: {err}", path.display()))
}

pub fn clear_default_profile(name: &str) -> std::result::Result<Option<PathBuf>, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = remove_default_profile(&text, name);
    if updated == text {
        return Ok(None);
    }

    write_config_text(&path, updated)?;
    Ok(Some(path))
}

fn remove_default_profile(text: &str, name: &str) -> String {
    let target = format!("default = \"{name}\"");
    let mut output = String::new();
    let mut in_profile = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            in_profile = trimmed == "[profile]";
        }

        if in_profile && trimmed == target {
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    output
}

fn upsert_default_profile(text: &str, name: &str) -> String {
    let mut output = String::new();
    let mut in_profile = false;
    let mut saw_profile = false;
    let mut wrote_default = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            if in_profile && !wrote_default {
                output.push_str(&format!("default = \"{name}\"\n"));
                wrote_default = true;
            }
            in_profile = trimmed == "[profile]";
            saw_profile |= in_profile;
        }

        if in_profile
            && trimmed
                .split_once('=')
                .is_some_and(|(key, _)| matches!(key.trim(), "default" | "path"))
        {
            if trimmed.starts_with("default") && !wrote_default {
                output.push_str(&format!("default = \"{name}\"\n"));
                wrote_default = true;
            }
            continue;
        }

        output.push_str(line);
        output.push('\n');
    }

    if in_profile && !wrote_default {
        output.push_str(&format!("default = \"{name}\"\n"));
    } else if !saw_profile {
        if !output.is_empty() && !output.ends_with("\n\n") {
            output.push('\n');
        }
        output.push_str("[profile]\n");
        output.push_str(&format!("default = \"{name}\"\n"));
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_profiles_reads_sorted_toml_stems() {
        let temp =
            std::env::temp_dir().join(format!("hyprmux-profiles-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).expect("tempdir");
        unsafe {
            std::env::set_var("XDG_CONFIG_HOME", &temp);
        }

        let profiles = temp.join("hyprmux/profiles");
        std::fs::create_dir_all(&profiles).expect("profiles dir");
        std::fs::write(profiles.join("beta.toml"), "version = 1\n").expect("beta");
        std::fs::write(profiles.join("alpha.toml"), "version = 1\n").expect("alpha");
        std::fs::write(profiles.join("notes.txt"), "skip").expect("txt");

        let listed = list_profiles();
        assert_eq!(
            listed
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        unsafe {
            std::env::remove_var("XDG_CONFIG_HOME");
        }
        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn profile_upsert_adds_missing_section() {
        assert_eq!(
            upsert_default_profile("scrollback = 100\n", "dev"),
            "scrollback = 100\n\n[profile]\ndefault = \"dev\"\n"
        );
    }

    #[test]
    fn profile_upsert_replaces_default_and_removes_path() {
        let updated = upsert_default_profile(
            "[profile]\npath = \"~/old.toml\"\ndefault = \"old\"\n\n[session]\nautosave = true\n",
            "dev",
        );
        assert_eq!(
            updated,
            "[profile]\ndefault = \"dev\"\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn remove_default_profile_strips_matching_entry() {
        let text = "[profile]\ndefault = \"dev\"\n\n[session]\nautosave = true\n";
        assert_eq!(
            remove_default_profile(text, "dev"),
            "[profile]\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn remove_default_profile_leaves_other_defaults() {
        let text = "[profile]\ndefault = \"work\"\n";
        assert_eq!(remove_default_profile(text, "dev"), text);
    }

    #[test]
    fn delete_profile_file_treats_missing_as_success() {
        let path = std::env::temp_dir().join(format!(
            "hyprmux-missing-profile-{}.toml",
            std::process::id()
        ));
        delete_profile_file(&path).expect("missing profile delete succeeds");
    }

    #[test]
    fn upsert_bool_in_section_replaces_and_preserves_comments() {
        let text = "# chrome prefs\n[pane]\nfocus_on_hover = true\n# keep\n";
        let updated = upsert_bool_in_section(text, "pane", "focus_on_hover", false);
        assert!(updated.contains("# chrome prefs"));
        assert!(updated.contains("focus_on_hover = false"));
        assert!(updated.contains("# keep"));
        assert!(!updated.contains("focus_on_hover = true"));
    }

    #[test]
    fn upsert_bool_in_section_appends_missing_section() {
        let updated = upsert_bool_in_section("", "pane", "show_workbar", true);
        assert_eq!(updated, "[pane]\nshow_workbar = true\n");
    }

    #[test]
    fn theme_upsert_adds_missing_section() {
        assert_eq!(
            upsert_theme_name("scrollback = 100\n", "lipan"),
            "scrollback = 100\n\n[theme]\nname = \"lipan\"\n"
        );
    }

    #[test]
    fn theme_upsert_replaces_name_and_removes_legacy_keys() {
        let updated = upsert_theme_name(
            "[theme]\npreset = \"dracula\"\npath = \"~/theme.toml\"\n\n[session]\nautosave = true\n",
            "my-nord",
        );
        assert_eq!(
            updated,
            "[theme]\nname = \"my-nord\"\n\n[session]\nautosave = true\n"
        );
    }
}
