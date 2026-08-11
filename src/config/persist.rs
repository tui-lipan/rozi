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

/// Persist a `[workbar.alert]` string key. The nested section is upserted like any other, so a
/// config that has never mentioned workbar alerts gains the header on first use.
pub fn persist_workbar_alert_string(
    key: &str,
    value: &str,
) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };

    let updated = upsert_value_in_section(&text, "workbar.alert", key, &format!("\"{value}\""));
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn persist_layout_default(
    kind: crate::state::LayoutKind,
) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };
    let updated =
        upsert_value_in_section(&text, "layout", "default", &format!("\"{}\"", kind.label()));
    write_config_text(&path, updated)?;
    Ok(path)
}

pub fn persist_sidebar_width(width: u16) -> std::result::Result<PathBuf, String> {
    persist_sidebar_value("width", &width.to_string())
}

pub fn persist_sidebar_split_ratio(ratio: f32) -> std::result::Result<PathBuf, String> {
    persist_sidebar_value("split_ratio", &format!("{ratio:.3}"))
}

pub fn persist_sidebar_split(split: bool) -> std::result::Result<PathBuf, String> {
    persist_sidebar_value("split", if split { "true" } else { "false" })
}

pub fn persist_sidebar_visible(visible: bool) -> std::result::Result<PathBuf, String> {
    persist_sidebar_value("visible", if visible { "true" } else { "false" })
}

pub fn persist_sidebar_panels(
    panels: &[Vec<super::schema::SidebarTabId>],
) -> std::result::Result<PathBuf, String> {
    let value = panels
        .iter()
        .map(|panel| {
            let tabs = panel
                .iter()
                .map(|id| toml::Value::String(id.as_str().to_string()).to_string())
                .collect::<Vec<_>>()
                .join(", ");
            format!("[{tabs}]")
        })
        .collect::<Vec<_>>()
        .join(", ");
    persist_sidebar_value("panels", &format!("[{value}]"))
}

fn persist_sidebar_value(key: &str, value: &str) -> std::result::Result<PathBuf, String> {
    let path = config_path();
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == io::ErrorKind::NotFound => String::new(),
        Err(err) => return Err(format!("Could not read config {}: {err}", path.display())),
    };
    let updated = upsert_value_in_section(&text, "sidebar", key, value);
    write_config_text(&path, updated)?;
    Ok(path)
}

fn upsert_bool_in_section(text: &str, section: &str, key: &str, value: bool) -> String {
    upsert_value_in_section(text, section, key, if value { "true" } else { "false" })
}

/// Insert or replace `key = <line_value>` inside `[section]`, creating the section at the end
/// of the file when it does not exist yet. `line_value` is written verbatim (already quoted for
/// strings, bare for bools/numbers). Replacing a multiline TOML value consumes its complete source
/// assignment rather than leaving continuation lines behind.
fn upsert_value_in_section(text: &str, section: &str, key: &str, line_value: &str) -> String {
    let section_header = format!("[{section}]");
    let mut output = String::new();
    let mut in_section = false;
    let mut saw_section = false;
    let mut wrote_key = false;
    let mut lines = text.lines().peekable();

    while let Some(line) = lines.next() {
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
            && let Some((candidate, old_value)) = trimmed.split_once('=')
            && candidate.trim() == key
        {
            if !wrote_key {
                output.push_str(&format!("{key} = {line_value}\n"));
                wrote_key = true;
            }
            consume_toml_value(old_value.trim(), &mut lines);
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

fn consume_toml_value<'a>(first_line: &str, lines: &mut std::iter::Peekable<std::str::Lines<'a>>) {
    let mut assignment = format!("value = {first_line}");
    let mut consumed_continuation = false;
    while toml::from_str::<toml::Value>(&assignment).is_err() {
        let Some(line) = lines.next() else {
            break;
        };
        consumed_continuation = true;
        assignment.push('\n');
        assignment.push_str(line);
    }

    // Older hyprmux builds replaced only the first line of a multiline array, producing a complete
    // inline assignment followed by the orphaned old array rows. Recognize that tail as an array
    // body and consume it too, allowing the next preference write to repair affected configs.
    let starts_orphaned_array = lines.peek().is_some_and(|line| {
        line.len() != line.trim_start().len() && line.trim_start().starts_with('[')
    });
    if !consumed_continuation && starts_orphaned_array {
        let probe = lines.clone();
        let mut orphaned = String::from("value = [");
        for (index, line) in probe.enumerate() {
            orphaned.push('\n');
            orphaned.push_str(line);
            if toml::from_str::<toml::Value>(&orphaned).is_ok() {
                for _ in 0..=index {
                    lines.next();
                }
                break;
            }
        }
    }
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

#[cfg(test)]
mod sidebar_persistence_tests {
    use super::*;

    #[test]
    fn panel_layout_serializes_as_valid_inline_arrays() {
        let panels = [
            vec![
                super::super::schema::SidebarTabId::new("panes"),
                super::super::schema::SidebarTabId::new("quoted-tab"),
            ],
            vec![super::super::schema::SidebarTabId::new("agents")],
        ];
        let value = panels
            .iter()
            .map(|panel| {
                let tabs = panel
                    .iter()
                    .map(|id| toml::Value::String(id.as_str().to_string()).to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("[{tabs}]")
            })
            .collect::<Vec<_>>()
            .join(", ");
        let text = upsert_value_in_section("", "sidebar", "panels", &format!("[{value}]"));
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(parsed["sidebar"]["panels"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn split_flag_is_independent_from_panel_layout() {
        let source = "[sidebar]\npanels = [[\"agents\"], [\"panes\"]]\nsplit = true\n";
        let updated = upsert_value_in_section(source, "sidebar", "split", "false");
        let parsed: toml::Value = toml::from_str(&updated).expect("updated config remains valid");
        assert_eq!(parsed["sidebar"]["split"].as_bool(), Some(false));
        assert_eq!(parsed["sidebar"]["panels"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn panel_layout_replaces_the_complete_multiline_value() {
        let source = format!(
            "{}\n[workbar]\nright = [\"session\"]\n",
            include_str!("../../examples/sidebar.toml")
        );
        let updated = upsert_value_in_section(
            &source,
            "sidebar",
            "panels",
            r#"[["agents", "panes", "files", "git"], ["dev", "branches", "sessions"]]"#,
        );

        assert_eq!(updated.matches("panels =").count(), 1, "{updated}");
        let parsed: toml::Value = toml::from_str(&updated).expect("updated config remains valid");
        assert_eq!(parsed["sidebar"]["panels"][1][2].as_str(), Some("sessions"));
        assert_eq!(parsed["sidebar"]["split_ratio"].as_float(), Some(0.62));
        assert_eq!(parsed["workbar"]["right"][0].as_str(), Some("session"));
    }

    #[test]
    fn panel_layout_repairs_an_orphaned_multiline_array_tail() {
        let source = r#"[sidebar]
panels = [["agents", "panes", "files", "git"], ["dev", "branches", "sessions"]]
  ["agents", "panes", "files", "git"],
  ["sessions", "dev", "branches"],
]
split_ratio = 0.62
"#;
        let updated = upsert_value_in_section(
            source,
            "sidebar",
            "panels",
            r#"[["agents", "files"], ["panes", "sessions", "dev", "branches"]]"#,
        );

        let parsed: toml::Value = toml::from_str(&updated).expect("corrupt config is repaired");
        assert_eq!(updated.matches("panels =").count(), 1, "{updated}");
        assert_eq!(parsed["sidebar"]["panels"][0][1].as_str(), Some("files"));
        assert_eq!(parsed["sidebar"]["panels"][1][3].as_str(), Some("branches"));
        assert_eq!(parsed["sidebar"]["split_ratio"].as_float(), Some(0.62));
    }
}

pub fn profiles_dir() -> PathBuf {
    config_home().join("profiles")
}

pub fn profile_path_for_name(name: &str) -> PathBuf {
    profiles_dir().join(format!("{name}.toml"))
}

pub fn list_profiles() -> Vec<ProfileEntry> {
    list_profiles_in(&profiles_dir())
}

/// The listing itself: `*.toml` stems, sorted, from one directory.
fn list_profiles_in(dir: &Path) -> Vec<ProfileEntry> {
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

/// The key of a `key = value` line, if it is one.
fn toml_key(line: &str) -> Option<&str> {
    line.split_once('=').map(|(key, _)| key.trim())
}

/// The unquoted string value of a `key = value` line, accepting either TOML quote style and
/// tolerating a trailing comment on a bare value.
fn toml_string_value(line: &str) -> Option<String> {
    let value = line.split_once('=')?.1.trim();
    match value.chars().next() {
        Some(quote @ ('"' | '\'')) => {
            let rest = &value[quote.len_utf8()..];
            let end = rest.find(quote)?;
            Some(rest[..end].to_string())
        }
        _ => Some(value.split('#').next()?.trim().to_string()),
    }
}

fn remove_default_profile(text: &str, name: &str) -> String {
    let mut output = String::new();
    let mut in_profile = false;

    for line in text.lines() {
        let trimmed = line.trim();
        let section_starts = trimmed.starts_with('[') && trimmed.ends_with(']');
        if section_starts {
            in_profile = trimmed == "[profile]";
        }

        // Match the parsed key and value, not one rendered spelling: a hand-written `default='dev'`
        // has to clear too, or the picker reports the default gone while the file puts it back on
        // the next launch. The value still has to match, so clearing `dev` leaves another default.
        if in_profile
            && toml_key(trimmed) == Some("default")
            && toml_string_value(trimmed).as_deref() == Some(name)
        {
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

        // Rewrite any existing `default` in place, whatever its spelling, and drop later duplicates.
        // Only that key is touched: every other line in `[profile]` is the user's and is preserved.
        if in_profile && toml_key(trimmed) == Some("default") {
            if !wrote_default {
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
        let profiles =
            std::env::temp_dir().join(format!("hyprmux-profiles-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profiles);
        std::fs::create_dir_all(&profiles).expect("profiles dir");
        std::fs::write(profiles.join("beta.toml"), "version = 1\n").expect("beta");
        std::fs::write(profiles.join("alpha.toml"), "version = 1\n").expect("alpha");
        std::fs::write(profiles.join("notes.txt"), "skip").expect("txt");

        let listed = list_profiles_in(&profiles);
        assert_eq!(
            listed
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            vec!["alpha", "beta"]
        );

        let _ = std::fs::remove_dir_all(&profiles);
    }

    #[test]
    fn profile_upsert_adds_missing_section() {
        assert_eq!(
            upsert_default_profile("scrollback = 100\n", "dev"),
            "scrollback = 100\n\n[profile]\ndefault = \"dev\"\n"
        );
    }

    #[test]
    fn profile_upsert_replaces_default_in_place() {
        let updated = upsert_default_profile(
            "[profile]\ndefault = \"old\"\n\n[session]\nautosave = true\n",
            "dev",
        );
        assert_eq!(
            updated,
            "[profile]\ndefault = \"dev\"\n\n[session]\nautosave = true\n"
        );
    }

    #[test]
    fn profile_upsert_rewrites_any_spelling_of_default() {
        assert_eq!(
            upsert_default_profile("[profile]\ndefault='old'  # pinned\n", "dev"),
            "[profile]\ndefault = \"dev\"\n"
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

    /// A hand-written default has to clear, or unsetting it in the picker leaves the file saying
    /// otherwise and the default returns on the next launch.
    #[test]
    fn remove_default_profile_strips_hand_written_spellings() {
        for text in [
            "[profile]\ndefault='dev'\n",
            "[profile]\ndefault=\"dev\"\n",
            "[profile]\n  default   =   \"dev\"   \n",
            "[profile]\ndefault = \"dev\" # pinned\n",
        ] {
            assert_eq!(remove_default_profile(text, "dev"), "[profile]\n", "{text}");
        }
    }

    /// `[profile]` denies unknown fields, so anything else under it is a key this build does not
    /// own yet - rewriting the default must not quietly delete it.
    #[test]
    fn profile_upsert_preserves_unrelated_profile_lines() {
        assert_eq!(
            upsert_default_profile("[profile]\n# keep me\npath = \"~/old.toml\"\n", "dev"),
            "[profile]\n# keep me\npath = \"~/old.toml\"\ndefault = \"dev\"\n"
        );
    }

    #[test]
    fn remove_default_profile_keeps_commented_out_default() {
        let text = "[profile]\n# default = \"dev\"\n";
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

    /// `[workbar.alert]` is the first nested section anything persists into, so pin that the
    /// dotted header is matched as a section of its own rather than folded into `[workbar]`.
    #[test]
    fn upsert_value_in_section_handles_the_nested_workbar_alert_table() {
        let created = upsert_value_in_section("", "workbar.alert", "mode", "\"static\"");
        assert_eq!(created, "[workbar.alert]\nmode = \"static\"\n");

        let text = "[workbar]\nclock_format = \"%H:%M\"\n\n[workbar.alert]\nbell = true\nmode = \"pulse\"\n";
        let updated = upsert_value_in_section(text, "workbar.alert", "mode", "\"off\"");
        assert!(updated.contains("clock_format = \"%H:%M\""));
        assert!(updated.contains("bell = true"));
        assert!(updated.contains("mode = \"off\""));
        assert!(!updated.contains("mode = \"pulse\""));

        // A `[workbar]` that has never mentioned alerts must not have the key spliced into it.
        let bare = upsert_value_in_section(
            "[workbar]\nclock_format = \"%H:%M\"\n",
            "workbar.alert",
            "mode",
            "\"off\"",
        );
        assert!(bare.contains("[workbar.alert]\nmode = \"off\""));
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
