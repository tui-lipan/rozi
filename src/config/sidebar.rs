use std::collections::HashSet;

use super::file::{SidebarFileConfig, SidebarTabSpec};
use super::input::parse_user_command_action;
use super::schema::{
    SIDEBAR_MAX_SPLIT_RATIO, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_COMMAND_INTERVAL_SECS,
    SIDEBAR_MIN_SPLIT_RATIO, SIDEBAR_MIN_WIDTH, SIDEBAR_TREE_MAX_ENTRIES_LIMIT, SidebarConfig,
    SidebarLauncherEntry, SidebarPosition, SidebarTab, SidebarTabId, SidebarTreeConfig,
    SidebarTreeRoot, SidebarTreeView,
};

const BUILTIN_TABS: &[&str] = &["agents", "panes", "sessions", "files", "git"];

/// The built-in tabs that a table form may *configure* rather than merely name. The other built-ins
/// take no options, so a table naming one is a mistake worth warning about.
fn tree_view(name: &str) -> Option<SidebarTreeView> {
    match name {
        "files" => Some(SidebarTreeView::Files),
        "git" => Some(SidebarTreeView::Changes),
        _ => None,
    }
}

fn build_tree_tab(
    view: SidebarTreeView,
    table: super::file::SidebarTabTableSpec,
    warnings: &mut Vec<String>,
) -> SidebarTab {
    let name = view.id();
    let mut config = SidebarTreeConfig::for_view(view);
    if table.entries.is_some() || table.command.is_some() || table.interval.is_some() {
        warnings.push(format!(
            "Sidebar tab `{name}` is a reserved built-in file tree; ignoring `entries`/`command`/`interval` (rename the tab to keep those as a custom tab)"
        ));
    }
    if let Some(root) = table.root {
        match SidebarTreeRoot::parse(&root) {
            Some(root) => config.root = root,
            None => warnings.push(format!(
                "Sidebar tab `{name}` root `{root}` is unknown (expected `cwd` or `repo`)"
            )),
        }
    }
    if let Some(show_hidden) = table.show_hidden {
        config.show_hidden = show_hidden;
    }
    if let Some(icons) = table.icons {
        config.icons = icons;
    }
    if let Some(explorer) = table.explorer {
        config.explorer = explorer;
    }
    if let Some(diff_stats) = table.diff_stats {
        config.diff_stats = diff_stats;
    }
    if let Some(max_entries) = table.max_entries {
        let clamped = max_entries.clamp(1, SIDEBAR_TREE_MAX_ENTRIES_LIMIT);
        if clamped != max_entries {
            warnings.push(format!(
                "Sidebar tab `{name}` max_entries {max_entries} out of range; clamped to {clamped}"
            ));
        }
        config.max_entries = clamped;
    }
    if let Some(action) = table.on_click {
        config.on_click = parse_sidebar_action(
            action,
            &format!("Sidebar tab `{name}` on_click"),
            "{path}",
            warnings,
        );
    }
    SidebarTab::Tree { view, config }
}

pub(super) fn apply_sidebar_config(
    sidebar: &mut SidebarConfig,
    raw: SidebarFileConfig,
    warnings: &mut Vec<String>,
) {
    let requested_panels = raw.panels;
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
    sidebar.panels = build_panels(&sidebar.tabs, requested_panels, warnings);
    sidebar.split = raw.split.unwrap_or(sidebar.panels.len() > 1);
    if sidebar.split && sidebar.panels.len() == 1 {
        sidebar.panels.push(Vec::new());
    }
    if let Some(split_ratio) = raw.split_ratio {
        if !split_ratio.is_finite() {
            warnings.push(format!(
                "Sidebar split_ratio {split_ratio} is not finite; keeping {}",
                sidebar.split_ratio
            ));
            return;
        }
        let clamped = split_ratio.clamp(SIDEBAR_MIN_SPLIT_RATIO, SIDEBAR_MAX_SPLIT_RATIO);
        if (clamped - split_ratio).abs() > f32::EPSILON {
            warnings.push(format!(
                "Sidebar split_ratio {split_ratio} out of range; clamped to {clamped}"
            ));
        }
        sidebar.split_ratio = clamped;
    }
}

fn build_panels(
    tabs: &[SidebarTab],
    requested: Option<Vec<Vec<String>>>,
    warnings: &mut Vec<String>,
) -> Vec<Vec<SidebarTabId>> {
    let ids: Vec<_> = tabs.iter().map(SidebarTab::id).collect();
    let Some(mut requested) = requested else {
        return vec![ids];
    };
    if requested.is_empty() {
        warnings.push("Sidebar panels must contain one or two panels; using one panel".to_string());
        return vec![ids];
    }
    if requested.len() > 2 {
        warnings.push(
            "Sidebar panels supports at most two panels; extra panels were ignored".to_string(),
        );
        requested.truncate(2);
    }

    let mut seen = HashSet::new();
    let mut panels = Vec::with_capacity(requested.len());
    for panel in requested {
        let mut resolved = Vec::new();
        for name in panel {
            let id = SidebarTabId::new(name.trim());
            if !ids.contains(&id) {
                warnings.push(format!(
                    "Unknown sidebar panel tab `{}`; skipped",
                    id.as_str()
                ));
            } else if !seen.insert(id.clone()) {
                warnings.push(format!(
                    "Sidebar panel tab `{}` appears more than once; duplicate skipped",
                    id.as_str()
                ));
            } else {
                resolved.push(id);
            }
        }
        panels.push(resolved);
    }

    let omitted: Vec<_> = ids.into_iter().filter(|id| !seen.contains(id)).collect();
    if !omitted.is_empty() {
        warnings.push(format!(
            "Sidebar panels omitted {}; appended to the first panel",
            omitted
                .iter()
                .map(|id| format!("`{}`", id.as_str()))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        panels[0].extend(omitted);
    }
    panels
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
                other => match tree_view(other) {
                    Some(view) => SidebarTab::Tree {
                        view,
                        config: SidebarTreeConfig::for_view(view),
                    },
                    None => {
                        warnings.push(format!("Unknown built-in sidebar tab `{name}`; skipped"));
                        continue;
                    }
                },
            },
            SidebarTabSpec::Table(table) => {
                let name = table.name.trim();
                if name.is_empty() {
                    warnings.push("Sidebar tab name must not be empty; skipped".to_string());
                    continue;
                }
                // A table naming a built-in file tree configures it; every other built-in name is
                // still reserved, since those tabs take no options.
                if let Some(view) = tree_view(name) {
                    let tab = build_tree_tab(view, *table, warnings);
                    let id = tab.id();
                    if !seen.insert(id.clone()) {
                        warnings.push(format!("Duplicate sidebar tab `{}`; skipped", id.as_str()));
                        continue;
                    }
                    tabs.push(tab);
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
                                    "{line}",
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
                                "{line}",
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

/// Parse a sidebar action, rejecting `placeholder` in `run`/`popup`. Substitution is only ever
/// performed into `send` text, which reaches the pane as literal keystrokes; a `run` or `popup`
/// command line is executed, and must never be assembled out of command output or a filename that
/// happens to live in the repository.
fn parse_sidebar_action(
    raw: super::file::UserCommandTableSpec,
    context: &str,
    placeholder: &str,
    warnings: &mut Vec<String>,
) -> Option<super::schema::UserCommandAction> {
    let action = parse_user_command_action(raw, context, warnings)?;
    if matches!(
        &action,
        super::schema::UserCommandAction::Run { command, .. }
            | super::schema::UserCommandAction::Popup { command, .. }
            if command.contains(placeholder)
    ) {
        warnings.push(format!(
            "{context} uses `{placeholder}` in run/popup; only send actions support this placeholder; skipped"
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
        assert!(!config.split);
        assert_eq!(config.split_ratio, 0.5);
        assert_eq!(
            config.tabs.iter().map(SidebarTab::id).collect::<Vec<_>>(),
            vec![
                SidebarTabId::new("agents"),
                SidebarTabId::new("panes"),
                SidebarTabId::new("sessions")
            ]
        );
        assert_eq!(config.panels.len(), 1);
        assert_eq!(
            config.panels[0],
            config.tabs.iter().map(SidebarTab::id).collect::<Vec<_>>()
        );
    }

    #[test]
    fn panels_assign_order_validate_ids_and_keep_omitted_tabs_reachable() {
        let (config, warnings) = parse(
            r#"
            tabs = ["agents", "panes", "sessions"]
            panels = [["panes"], ["agents", "bogus", "agents"]]
            split_ratio = 0.7
            "#,
        );
        assert_eq!(config.split_ratio, 0.7);
        assert_eq!(
            config.panels,
            vec![
                vec![SidebarTabId::new("panes"), SidebarTabId::new("sessions")],
                vec![SidebarTabId::new("agents")],
            ]
        );
        assert!(warnings.iter().any(|warning| warning.contains("bogus")));
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("more than once"))
        );
        assert!(warnings.iter().any(|warning| warning.contains("omitted")));
    }

    #[test]
    fn split_controls_presentation_without_discarding_panel_placement() {
        let (config, warnings) = parse(
            r#"
            tabs = ["agents", "panes", "sessions"]
            panels = [["agents"], ["panes", "sessions"]]
            split = false
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(!config.split);
        assert_eq!(config.panels.len(), 2);

        let (config, warnings) = parse(
            r#"
            tabs = ["agents", "panes", "sessions"]
            panels = [["agents"], ["panes", "sessions"]]
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.split);

        let (config, warnings) = parse(
            r#"
            tabs = ["agents", "panes", "sessions"]
            split = true
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.split);
        assert_eq!(config.panels.len(), 2);
        assert!(config.panels[1].is_empty());
    }

    fn tree(config: &SidebarConfig, id: &str) -> SidebarTreeConfig {
        config
            .tabs
            .iter()
            .find_map(|tab| match tab {
                SidebarTab::Tree { config, .. } if tab.id() == SidebarTabId::new(id) => {
                    Some(config.clone())
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("`{id}` is a file tree tab"))
    }

    /// Bare `files` / `git` names work, and each carries the defaults that make its view useful:
    /// browsing is rooted at the pane's directory without diff noise, while the changes view is
    /// repo-rooted with diff stats, since changes elsewhere in the repo matter from a subdirectory.
    #[test]
    fn file_tree_tabs_parse_by_name_with_per_view_defaults() {
        let (config, warnings) = parse(r#"tabs = ["files", "git"]"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            config
                .tabs
                .iter()
                .map(SidebarTab::label)
                .collect::<Vec<_>>(),
            vec!["Files", "Git"]
        );

        let files = tree(&config, "files");
        assert_eq!(files.root, SidebarTreeRoot::Cwd);
        assert!(!files.diff_stats);
        let git = tree(&config, "git");
        assert_eq!(git.root, SidebarTreeRoot::Repo);
        assert!(git.diff_stats);
        // Both default to typing the activated path at the prompt.
        assert_eq!(
            files.on_click,
            Some(super::super::schema::UserCommandAction::Send(
                "{path}".into()
            ))
        );

        // Only the file trees scroll internally; every other tab is wrapped by the sidebar.
        assert!(config.tabs.iter().all(SidebarTab::scrolls_itself));
        assert!(!SidebarTab::Panes.scrolls_itself());
    }

    #[test]
    fn file_tree_table_form_overrides_options() {
        let (config, warnings) = parse(
            r#"
            tabs = [{ name = "files", label = "", show_hidden = true, icons = true, explorer = true, diff_stats = true, max_entries = 50, root = "repo", on_click = { send = "nvim {path}\n" } }]
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let files = tree(&config, "files");
        assert_eq!(files.root, SidebarTreeRoot::Repo);
        assert!(files.show_hidden && files.icons && files.explorer && files.diff_stats);
        assert_eq!(files.max_entries, 50);
        assert_eq!(
            files.on_click,
            Some(super::super::schema::UserCommandAction::Send(
                "nvim {path}\n".into()
            ))
        );
        // A built-in keeps its own label; the table's empty label is not an error here.
        assert_eq!(config.tabs[0].label(), "Files");
    }

    /// `{path}` may only reach the pane as literal keystrokes. A `run`/`popup` command line is
    /// executed, and a filename in the repository must never be able to compose one.
    #[test]
    fn file_tree_rejects_path_placeholder_in_executed_actions() {
        let (config, warnings) =
            parse(r#"tabs = [{ name = "files", label = "", on_click = { run = "rm {path}" } }]"#);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("{path}"), "{warnings:?}");
        assert_eq!(tree(&config, "files").on_click, None);

        // The supported way to scope a command to the clicked file: the path is referenced through
        // the environment, so nothing is spliced into the command line and it is accepted.
        let (config, warnings) = parse(
            r#"tabs = [{ name = "git", label = "", on_click = { run = "lazygit -f \"$HYPRMUX_FILE\"" } }]"#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            tree(&config, "git").on_click,
            Some(super::super::schema::UserCommandAction::run(
                "lazygit -f \"$HYPRMUX_FILE\""
            ))
        );
    }

    #[test]
    fn file_tree_options_are_validated_and_clamped() {
        let (config, warnings) = parse(
            r#"
            tabs = [
              { name = "files", label = "", root = "elsewhere", max_entries = 0 },
              { name = "git", label = "", command = "ls" },
            ]
            "#,
        );
        assert_eq!(tree(&config, "files").root, SidebarTreeRoot::Cwd);
        assert_eq!(tree(&config, "files").max_entries, 1);
        assert!(
            warnings.iter().any(|w| w.contains("elsewhere")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("max_entries")),
            "{warnings:?}"
        );
        // A file tree takes no `command`; saying so beats silently ignoring it.
        assert!(
            warnings.iter().any(|w| w.contains("built-in file tree")),
            "{warnings:?}"
        );
    }

    /// The other built-ins take no options, so their names stay reserved for the table form.
    #[test]
    fn non_tree_builtin_names_remain_reserved() {
        let (config, warnings) =
            parse(r#"tabs = ["files", { name = "panes", label = "Mine", command = "ls" }]"#);
        assert!(
            warnings.iter().any(|w| w.contains("reserved")),
            "{warnings:?}"
        );
        assert_eq!(config.tabs.len(), 1);
    }

    #[test]
    fn parses_builtin_launcher_and_command_tabs() {
        // TOML 1.1 allows multiline inline tables (and trailing commas), which is what real
        // sidebar configs use once tabs grow beyond a one-liner.
        let (config, warnings) = parse(
            r#"
            visible = true
            width = 40
            position = "right"
            tabs = [
              "panes",
              { name = "deploy", label = "Deploy", entries = [
                { label = "Build", run = "cargo build" },
                { label = "Test", send = "cargo test\n" },
                { label = "Logs", popup = "journalctl -f" },
              ] },
              {
                name = "todos",
                label = "Todos",
                command = "task list --plain",
                interval = 30,
                on_click = { send = "task view {line}\n" },
              },
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
