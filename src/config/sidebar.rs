use std::collections::HashSet;

use super::file::{SidebarFileConfig, SidebarTabSpec};
use super::input::parse_user_command_action;
use super::schema::{
    SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_COMMAND_INTERVAL_SECS, SIDEBAR_MIN_WIDTH, SidebarConfig,
    SidebarLauncherEntry, SidebarPosition, SidebarTab, SidebarTabId,
};

const BUILTIN_TABS: &[&str] = &["agents", "panes", "sessions"];

pub(super) fn apply_sidebar_config(
    sidebar: &mut SidebarConfig,
    raw: SidebarFileConfig,
    warnings: &mut Vec<String>,
) {
    if let Some(visible) = raw.visible {
        sidebar.visible = visible;
    }
    if let Some(width) = raw.width {
        let clamped = width.clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
        if clamped != width {
            warnings.push(format!(
                "Sidebar width {width} out of range; clamped to {clamped}"
            ));
        }
        sidebar.width = clamped;
    }
    if let Some(position) = raw.position {
        match SidebarPosition::parse(&position) {
            Some(position) => sidebar.position = position,
            None => warnings.push(format!(
                "Ignored unknown sidebar.position `{position}` (expected `left` or `right`)"
            )),
        }
    }
    if let Some(tabs) = raw.tabs {
        sidebar.tabs = build_tabs(tabs, warnings);
    }
}

fn build_tabs(raw: Vec<SidebarTabSpec>, warnings: &mut Vec<String>) -> Vec<SidebarTab> {
    let mut seen = HashSet::new();
    let mut tabs = Vec::new();
    for spec in raw {
        let tab = match spec {
            SidebarTabSpec::Name(name) => match name.trim().to_ascii_lowercase().as_str() {
                "agents" => SidebarTab::Agents,
                "panes" => SidebarTab::Panes,
                "sessions" => SidebarTab::Sessions,
                _ => {
                    warnings.push(format!("Unknown built-in sidebar tab `{name}`; skipped"));
                    continue;
                }
            },
            SidebarTabSpec::Table(table) => {
                let name = table.name.trim();
                if name.is_empty() {
                    warnings.push("Sidebar tab name must not be empty; skipped".to_string());
                    continue;
                }
                if BUILTIN_TABS.contains(&name) {
                    warnings.push(format!("Sidebar tab name `{name}` is reserved; skipped"));
                    continue;
                }
                let label = table.label.trim();
                if label.is_empty() {
                    warnings.push(format!(
                        "Sidebar tab `{name}` label must not be empty; skipped"
                    ));
                    continue;
                }
                match (table.entries, table.command) {
                    (Some(entries), None) => {
                        let entries = entries
                            .into_iter()
                            .filter_map(|entry| {
                                let label = entry.label.trim().to_string();
                                if label.is_empty() {
                                    warnings.push(format!(
                                        "Sidebar tab `{name}` has an entry with an empty label; skipped"
                                    ));
                                    return None;
                                }
                                let action = parse_sidebar_action(
                                    entry.action(),
                                    &format!("Sidebar entry `{label}` in `{name}`"),
                                    warnings,
                                )?;
                                Some(SidebarLauncherEntry { label, action })
                            })
                            .collect();
                        SidebarTab::Launcher {
                            name: SidebarTabId::new(name),
                            label: label.to_string(),
                            entries,
                        }
                    }
                    (None, Some(command)) if !command.trim().is_empty() => {
                        let requested = table.interval.unwrap_or(30);
                        let interval_secs = requested.max(SIDEBAR_MIN_COMMAND_INTERVAL_SECS);
                        if interval_secs != requested {
                            warnings.push(format!(
                                "Sidebar tab `{name}` interval {requested}s is below the minimum; clamped to {interval_secs}s"
                            ));
                        }
                        let on_click = table.on_click.and_then(|action| {
                            parse_sidebar_action(
                                action,
                                &format!("Sidebar tab `{name}` on_click"),
                                warnings,
                            )
                        });
                        SidebarTab::Command {
                            name: SidebarTabId::new(name),
                            label: label.to_string(),
                            command: command.trim().to_string(),
                            interval_secs,
                            on_click,
                        }
                    }
                    _ => {
                        warnings.push(format!(
                            "Sidebar tab `{name}` needs exactly one of `entries` or `command`; skipped"
                        ));
                        continue;
                    }
                }
            }
        };
        let id = tab.id();
        if !seen.insert(id.clone()) {
            warnings.push(format!("Duplicate sidebar tab `{}`; skipped", id.as_str()));
            continue;
        }
        tabs.push(tab);
    }
    tabs
}

fn parse_sidebar_action(
    raw: super::file::UserCommandTableSpec,
    context: &str,
    warnings: &mut Vec<String>,
) -> Option<super::schema::UserCommandAction> {
    let action = parse_user_command_action(raw, context, warnings)?;
    if matches!(
        &action,
        super::schema::UserCommandAction::Run(command)
            | super::schema::UserCommandAction::Popup(command)
            if command.contains("{line}")
    ) {
        warnings.push(format!(
            "{context} uses `{{line}}` in run/popup; only send actions support this placeholder; skipped"
        ));
        None
    } else {
        Some(action)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> (SidebarConfig, Vec<String>) {
        let raw = toml::from_str(text).expect("sidebar config parses");
        let mut config = SidebarConfig::default();
        let mut warnings = Vec::new();
        apply_sidebar_config(&mut config, raw, &mut warnings);
        (config, warnings)
    }

    #[test]
    fn defaults_match_documented_schema() {
        let config = SidebarConfig::default();
        assert!(!config.visible);
        assert_eq!(config.width, 32);
        assert_eq!(config.position, SidebarPosition::Left);
        assert_eq!(
            config.tabs.iter().map(SidebarTab::id).collect::<Vec<_>>(),
            vec![
                SidebarTabId::new("agents"),
                SidebarTabId::new("panes"),
                SidebarTabId::new("sessions")
            ]
        );
    }

    #[test]
    fn parses_builtin_launcher_and_command_tabs() {
        let (config, warnings) = parse(
            r#"
            visible = true
            width = 40
            position = "right"
            tabs = [
              "panes",
              { name = "deploy", label = "Deploy", entries = [{ label = "Build", run = "cargo build" }, { label = "Test", send = "cargo test\n" }, { label = "Logs", popup = "journalctl -f" }] },
              { name = "todos", label = "Todos", command = "task list --plain", interval = 30, on_click = { send = "task view {line}\n" } }
            ]
        "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.visible);
        assert_eq!(config.position, SidebarPosition::Right);
        assert_eq!(config.tabs.len(), 3);
        assert!(
            matches!(&config.tabs[1], SidebarTab::Launcher { entries, .. } if entries.len() == 3)
        );
        assert!(matches!(
            &config.tabs[2],
            SidebarTab::Command {
                interval_secs: 30,
                on_click: Some(_),
                ..
            }
        ));
    }

    #[test]
    fn semantic_errors_warn_and_skip_only_invalid_items() {
        let (config, warnings) = parse(
            r#"
            width = 2
            tabs = ["bogus", "panes", "panes",
              { name = "panes", label = "Reserved", entries = [] },
              { name = "both", label = "Both", entries = [], command = "date" },
              { name = "launch", label = "Launch", entries = [
                { label = "Bad", run = "one", send = "two" },
                { label = "Good", run = "date" }
              ] }
            ]
        "#,
        );
        assert_eq!(config.width, SIDEBAR_MIN_WIDTH);
        assert_eq!(config.tabs.len(), 2);
        assert!(
            matches!(&config.tabs[1], SidebarTab::Launcher { entries, .. } if entries.len() == 1)
        );
        assert!(warnings.len() >= 5, "{warnings:?}");
    }

    #[test]
    fn unknown_table_fields_are_parse_errors() {
        assert!(
            toml::from_str::<SidebarFileConfig>(
                r#"tabs = [{ name = "x", label = "X", entries = [], typo = true }]"#
            )
            .is_err()
        );
    }

    #[test]
    fn line_placeholder_is_allowed_only_for_send_actions() {
        let (config, warnings) = parse(
            r#"tabs = [{ name = "rows", label = "Rows", command = "printf x", on_click = { run = "show {line}" } }]"#,
        );
        assert!(matches!(
            &config.tabs[0],
            SidebarTab::Command { on_click: None, .. }
        ));
        assert!(warnings.iter().any(|warning| warning.contains("only send")));

        let (config, warnings) = parse(
            r#"tabs = [{ name = "rows", label = "Rows", command = "printf x", on_click = { send = "show {line}\n" } }]"#,
        );
        assert!(warnings.is_empty());
        assert!(matches!(
            &config.tabs[0],
            SidebarTab::Command {
                on_click: Some(crate::config::UserCommandAction::Send(_)),
                ..
            }
        ));
    }
}
