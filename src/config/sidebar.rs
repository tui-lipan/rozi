use std::collections::{BTreeMap, BTreeSet, HashSet};

use super::file::{SidebarFileConfig, SidebarTabSpec};
use super::input::parse_user_command_action;
use super::schema::{
    ActivityWorktrees, SIDEBAR_MAX_SPLIT_RATIO, SIDEBAR_MIN_COMMAND_INTERVAL_SECS,
    SIDEBAR_MIN_SPLIT_RATIO, SIDEBAR_TREE_MAX_ENTRIES_LIMIT, SidebarActivityConfig, SidebarConfig,
    SidebarLauncherEntry, SidebarPosition, SidebarTab, SidebarTabId, SidebarTreeConfig,
    SidebarTreeRoot, SidebarTreeView,
};
use super::setting_schema::{SettingSpec, SettingValue, SettingsGroup, SettingsOwner};

const BUILTIN_TABS: &[&str] = &[
    "activity",
    "agents",
    "panes",
    "sessions",
    "files",
    "git",
    "worktrees",
    "projects",
];

/// A tree tab's setting that is an action rather than a value, so it is written in `config.toml`
/// and never offered in the settings picker.
const TREE_ON_CLICK: &str = "on_click";

fn tree_view(name: &str) -> Option<SidebarTreeView> {
    match name {
        "files" => Some(SidebarTreeView::Files),
        "git" => Some(SidebarTreeView::Changes),
        _ => None,
    }
}

fn builtin_tab(name: &str) -> Option<SidebarTab> {
    Some(match name {
        "activity" => SidebarTab::activity(),
        "agents" => SidebarTab::Agents,
        "panes" => SidebarTab::Panes,
        "sessions" => SidebarTab::Sessions,
        "worktrees" => SidebarTab::Worktrees,
        "projects" => SidebarTab::Projects,
        other => SidebarTab::tree(tree_view(other)?),
    })
}

/// The settings a built-in tab offers, in the order its settings picker lists them. Empty for a
/// tab with none.
///
/// This is the whole contract for a built-in tab's settings: `[sidebar.settings.<tab>]` is checked
/// against it, the picker is built from it, and the tab's typed config is read out of the values
/// it admits. A new setting is a line here and the code that reads it.
pub fn builtin_tab_settings(id: &str) -> Vec<SettingSpec> {
    let tree = |view: SidebarTreeView| {
        let defaults = SidebarTreeConfig::for_view(view);
        let root = match defaults.root {
            SidebarTreeRoot::Cwd => "cwd",
            SidebarTreeRoot::Repo => "repo",
        };
        let mut specs = vec![
            SettingSpec::toggle("explorer", "Search bar", defaults.explorer),
            SettingSpec::toggle("show_hidden", "Hidden files", defaults.show_hidden),
            SettingSpec::choice(
                "root",
                "Root",
                root,
                &[("cwd", "Pane directory"), ("repo", "Repository")],
            ),
            SettingSpec::toggle("diff_stats", "Line counts", defaults.diff_stats),
        ];
        if view == SidebarTreeView::Changes {
            specs.push(SettingSpec::toggle(
                "compact_dirs",
                "Compact directories",
                defaults.compact_dirs,
            ));
        }
        specs.push(SettingSpec::integer(
            "max_entries",
            "Entry limit",
            defaults.max_entries as i64,
            1,
            SIDEBAR_TREE_MAX_ENTRIES_LIMIT as i64,
        ));
        specs
    };
    match id {
        "activity" => vec![SettingSpec::choice(
            "worktrees",
            "Worktrees",
            "separate",
            &[("separate", "Separate"), ("repository", "By repository")],
        )],
        other => tree_view(other).map(tree).unwrap_or_default(),
    }
}

/// The Worktrees tab's settings. They live in `[worktrees]`, which the CLI reads too, not in
/// `[sidebar.settings.worktrees]`, so they are declared apart from [`builtin_tab_settings`].
pub fn worktrees_tab_settings() -> Vec<SettingSpec> {
    vec![
        SettingSpec::flags(
            "delete_branch",
            "Delete branch on remove",
            &[("local", "Local"), ("remote", "Remote")],
        )
        .with_description(
            "Which copies of a removed checkout's branch to delete too. Only merged branches are deleted.",
        ),
    ]
}

/// The Worktrees tab's settings group at the values `[worktrees]` holds. `set` is whether
/// `config.toml` names `delete_branch`.
pub(crate) fn worktrees_group(
    deletion: crate::git::worktrees::BranchDeletion,
    set: bool,
) -> SettingsGroup {
    let branches = [(deletion.local, "local"), (deletion.remote, "remote")]
        .into_iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name.to_string())
        .collect();
    SettingsGroup {
        owner: SettingsOwner::Worktrees,
        specs: worktrees_tab_settings(),
        values: BTreeMap::from([("delete_branch".to_string(), SettingValue::List(branches))]),
        overridden: if set {
            std::collections::BTreeSet::from(["delete_branch".to_string()])
        } else {
            std::collections::BTreeSet::new()
        },
    }
}

/// Read `[sidebar.settings.<tab>]` for one built-in tab against its declared settings.
///
/// An unknown key, a value of the wrong type, and an unknown choice are reported and leave the
/// setting at its default; an integer out of range is clamped. A tree tab's `on_click` action is
/// returned separately, since it is not a value a picker edits.
fn builtin_group(
    id: &str,
    specs: Vec<SettingSpec>,
    raw: Option<&toml::Value>,
    warnings: &mut Vec<String>,
) -> (SettingsGroup, Option<super::file::UserCommandTableSpec>) {
    let mut values: BTreeMap<String, SettingValue> = specs
        .iter()
        .map(|spec| (spec.key.clone(), spec.default.clone()))
        .collect();
    let mut overridden = BTreeSet::new();
    let mut on_click = None;
    let section = format!("sidebar.settings.{id}");
    match raw.map(|raw| raw.as_table()) {
        None => {}
        Some(None) => warnings.push(format!("`[{section}]` must be a table; ignored")),
        Some(Some(table)) => {
            for (key, value) in table {
                if key == TREE_ON_CLICK && tree_view(id).is_some() {
                    match value.clone().try_into() {
                        Ok(action) => on_click = Some(action),
                        Err(error) => {
                            warnings.push(format!("`{section}.{key}` is invalid: {error}"))
                        }
                    }
                    continue;
                }
                let Some(spec) = specs.iter().find(|spec| &spec.key == key) else {
                    warnings.push(format!(
                        "Sidebar tab `{id}` has no setting `{key}`; ignored"
                    ));
                    continue;
                };
                overridden.insert(key.clone());
                if let Some(value) =
                    spec.check_config_value(value, &format!("{section}.{key}"), warnings)
                {
                    values.insert(key.clone(), value);
                }
            }
        }
    }
    let group = SettingsGroup {
        owner: SettingsOwner::SidebarTab(id.to_string()),
        specs,
        values,
        overridden,
    };
    (group, on_click)
}

fn activity_config(group: &SettingsGroup) -> SidebarActivityConfig {
    let mut config = SidebarActivityConfig::default();
    if let Some(worktrees) = group
        .value("worktrees")
        .and_then(SettingValue::as_str)
        .and_then(ActivityWorktrees::parse)
    {
        config.worktrees = worktrees;
    }
    config
}

fn tree_config(
    view: SidebarTreeView,
    group: &SettingsGroup,
    on_click: Option<super::file::UserCommandTableSpec>,
    warnings: &mut Vec<String>,
) -> SidebarTreeConfig {
    let mut config = SidebarTreeConfig::for_view(view);
    let flag = |key: &str| group.value(key).and_then(SettingValue::as_bool);
    if let Some(root) = group
        .value("root")
        .and_then(SettingValue::as_str)
        .and_then(SidebarTreeRoot::parse)
    {
        config.root = root;
    }
    if let Some(explorer) = flag("explorer") {
        config.explorer = explorer;
    }
    if let Some(show_hidden) = flag("show_hidden") {
        config.show_hidden = show_hidden;
    }
    if let Some(diff_stats) = flag("diff_stats") {
        config.diff_stats = diff_stats;
    }
    if let Some(compact_dirs) = flag("compact_dirs") {
        config.compact_dirs = compact_dirs;
    }
    if let Some(max_entries) = group
        .value("max_entries")
        .and_then(SettingValue::as_integer)
    {
        config.max_entries = usize::try_from(max_entries).unwrap_or(config.max_entries);
    }
    if let Some(action) = on_click {
        config.on_click = parse_sidebar_action(
            action,
            &format!("Sidebar tab `{}` on_click", view.id()),
            "{path}",
            warnings,
        );
    }
    config
}

/// Apply `[sidebar.settings]` to the built-in tabs in `tabs`, returning each configurable tab's
/// settings group. A table for a tab that is absent or takes no settings is reported.
pub(crate) fn apply_builtin_settings(
    tabs: &mut [SidebarTab],
    raw: &BTreeMap<String, toml::Value>,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, SettingsGroup> {
    let mut groups = BTreeMap::new();
    for tab in tabs.iter_mut() {
        let id = tab.id().as_str().to_string();
        let specs = builtin_tab_settings(&id);
        if specs.is_empty() {
            continue;
        }
        let (group, on_click) = builtin_group(&id, specs, raw.get(&id), warnings);
        match tab {
            SidebarTab::Activity { config } => *config = activity_config(&group),
            SidebarTab::Tree { view, config } => {
                *config = tree_config(*view, &group, on_click, warnings);
            }
            _ => {}
        }
        groups.insert(id, group);
    }
    for id in raw.keys() {
        if groups.contains_key(id) {
            continue;
        }
        if BUILTIN_TABS.contains(&id.as_str()) && builtin_tab_settings(id).is_empty() {
            warnings.push(format!(
                "Sidebar tab `{id}` has no settings; ignoring `[sidebar.settings.{id}]`"
            ));
        } else if BUILTIN_TABS.contains(&id.as_str()) {
            // Configured, but not in `tabs`: nothing to apply it to, and nothing wrong either.
        } else if let Some((extension, _)) = id.split_once('.') {
            warnings.push(format!(
                "`[sidebar.settings.\"{id}\"]` is ignored; an extension tab's settings live in `[extensions.{extension}]`"
            ));
        } else {
            warnings.push(format!(
                "`[sidebar.settings.{id}]` names no built-in tab; ignored"
            ));
        }
    }
    groups
}

/// What installed extensions add to the sidebar. Merged inside the sidebar pass rather than after
/// it, because placement is resolved there and a tab that arrived too late to be placed would be
/// unreachable.
#[derive(Default)]
pub(crate) struct SidebarContributions {
    pub(crate) tabs: Vec<SidebarTab>,
    pub(crate) suggestions: BTreeMap<String, super::schema::SidebarTabLocation>,
    pub(crate) presets: Vec<super::schema::SidebarLayoutPreset>,
    /// The settings each extension tab offers, by namespaced tab id.
    pub(crate) tab_settings: BTreeMap<String, SettingsGroup>,
}

/// Apply the `[sidebar]` table, then let installed extensions add their own tabs.
///
/// Extension tabs are appended to the catalog before placement is resolved, so a tab the user has
/// already dragged somewhere keeps that spot and a new one is reachable without being placed by
/// hand. They can only ever add: a namespaced id cannot collide with a built-in, and a config tab
/// claiming the same id wins.
pub(super) fn apply_sidebar_config(
    sidebar: &mut SidebarConfig,
    raw: SidebarFileConfig,
    contributions: SidebarContributions,
    warnings: &mut Vec<String>,
) {
    let SidebarContributions {
        tabs: extension_tabs,
        suggestions,
        presets,
        tab_settings: extension_tab_settings,
    } = contributions;
    sidebar.presets = presets;
    let legacy = raw.visible.is_some()
        || raw.width.is_some()
        || raw.position.is_some()
        || raw.panels.is_some()
        || raw.split.is_some()
        || raw.split_ratio.is_some();
    if let Some(background_follows_canvas) = raw.background_follows_canvas {
        sidebar.background_follows_canvas = background_follows_canvas;
    }
    if let Some(gap) = raw.gap {
        sidebar.gap = gap;
    }
    if let Some(background) = raw.background {
        sidebar.background = background;
    }
    if let Some(tab_style) = raw.tab_style.as_deref() {
        match crate::state::parse_cap_style(tab_style) {
            Some(tui_lipan::prelude::CapStyle::Half) => warnings.push(format!(
                "Ignored sidebar.tab_style \"{tab_style}\" (half block is not available for tab bars)"
            )),
            Some(style) => sidebar.tab_style = style,
            None => warnings.push(format!(
                "Ignored unknown sidebar.tab_style \"{tab_style}\" (expected one of: padded, round, arrow)"
            )),
        }
    }
    if let Some(tabs) = raw.tabs {
        sidebar.tabs = build_tabs(tabs, warnings);
    }
    sidebar.tab_settings = apply_builtin_settings(
        &mut sidebar.tabs,
        &raw.settings.unwrap_or_default(),
        warnings,
    );
    for tab in extension_tabs {
        let id = tab.id();
        if sidebar.tabs.iter().any(|existing| existing.id() == id) {
            warnings.push(format!(
                "Sidebar tab `{}` already configured; extension definition skipped",
                id.as_str()
            ));
        } else {
            if let Some(group) = extension_tab_settings.get(id.as_str()) {
                sidebar
                    .tab_settings
                    .insert(id.as_str().to_string(), group.clone());
            }
            sidebar.tabs.push(tab);
        }
    }
    if let Some(layout) = raw.layout {
        if legacy {
            warnings.push("sidebar.layout overrides legacy sidebar geometry keys; remove visible/width/position/panels/split/split_ratio".into());
        }
        sidebar.layout = layout.layout;
        if let Some(docks) = layout.migrated_docks {
            sidebar.startup = docks.iter().any(|shown| *shown);
            sidebar.migrated_docks = sidebar.startup.then_some(docks);
            warnings.push("Legacy sidebar dock visible flags migrated to sidebar.startup and remembered docks; the next layout save writes the new schema".into());
        }
    } else if legacy {
        let mut layout = super::schema::SidebarDockLayout::default();
        let side = raw
            .position
            .as_deref()
            .and_then(SidebarPosition::parse)
            .unwrap_or_default();
        if side == SidebarPosition::Right {
            std::mem::swap(&mut layout.left, &mut layout.right);
        }
        let dock = layout.dock_mut(side);
        sidebar.startup = raw.visible.unwrap_or(false);
        sidebar.migrated_docks = sidebar.startup.then_some([
            side == SidebarPosition::Left,
            side == SidebarPosition::Right,
        ]);
        dock.width = raw.width.unwrap_or(32);
        if let Some(panels) = raw.panels {
            dock.panels = panels
                .into_iter()
                .map(|tabs| super::schema::SidebarDockPanel { tabs, weight: 1.0 })
                .collect();
        }
        dock.panel_count = if raw.split == Some(false) {
            1
        } else {
            dock.panels.len().clamp(1, 3)
        };
        if dock.panels.len() == 2 {
            let ratio = raw
                .split_ratio
                .filter(|r| r.is_finite())
                .unwrap_or(0.4)
                .clamp(SIDEBAR_MIN_SPLIT_RATIO, SIDEBAR_MAX_SPLIT_RATIO);
            dock.panels[0].weight = ratio;
            dock.panels[1].weight = 1.0 - ratio;
        }
        sidebar.layout = layout;
        warnings.push("Legacy sidebar geometry migrated in memory to sidebar.layout; the next layout save writes the new schema. See docs/sidebar.md".into());
    }
    if let Some(startup) = raw.startup {
        sidebar.startup = startup;
    }
    sidebar.layout.validate(warnings);
    // A tab nobody placed yet - a built-in added by a newer rozi, a new extension tab - joins the
    // dock you use: the left one, unless only the right one holds tabs. Otherwise a new tab would
    // open a dock someone who keeps everything on the right never asked for.
    let holds_tabs = |side| {
        sidebar
            .layout
            .dock(side)
            .panels
            .iter()
            .any(|panel| !panel.tabs.is_empty())
    };
    let home = if !holds_tabs(SidebarPosition::Left) && holds_tabs(SidebarPosition::Right) {
        SidebarPosition::Right
    } else {
        SidebarPosition::Left
    };
    for tab in &sidebar.tabs {
        let id = tab.id();
        if sidebar.layout.location(id.as_str()).is_none() {
            let panel = if ["files", "git", "worktrees", "projects"].contains(&id.as_str()) {
                1
            } else {
                0
            };
            if let Some(location) = suggestions.get(id.as_str()) {
                sidebar.layout.place(
                    id.as_str(),
                    SidebarPosition::parse(&location.dock).unwrap(),
                    location.panel - 1,
                );
            } else {
                sidebar.layout.place(id.as_str(), home, panel);
            }
        }
    }
}

/// Cluster launcher entries under their groups so the stored order is already display order, and
/// the rows only have to notice where the group changes. Section order is first appearance, entry order within a
/// section is config order, and ungrouped entries lead because they are the run that sits above the
/// first header.
fn cluster_by_group(mut entries: Vec<SidebarLauncherEntry>) -> Vec<SidebarLauncherEntry> {
    let mut order: Vec<Option<String>> = Vec::new();
    for entry in &entries {
        if !order.contains(&entry.group) {
            order.push(entry.group.clone());
        }
    }
    // Stable, so this only lifts each entry to its section without disturbing the entries around it.
    entries.sort_by_key(|entry| match &entry.group {
        None => 0,
        group => 1 + order.iter().position(|known| known == group).unwrap_or(0),
    });
    entries
}

/// The fields a custom sidebar tab is built from, shared by `config.toml`'s tab tables and an
/// extension manifest's `[[sidebar_tabs]]` so both forms accept exactly the same tab.
pub(super) struct CustomTabParts {
    pub(super) label: String,
    /// Environment the finished tab's processes inherit. Empty for a `config.toml` tab.
    pub(super) env: Vec<(String, String)>,
    pub(super) entries: Option<Vec<super::file::SidebarLauncherEntrySpec>>,
    pub(super) command: Option<String>,
    pub(super) interval: Option<u64>,
    pub(super) on_click: Option<super::file::UserCommandTableSpec>,
    pub(super) group_prefix: Option<String>,
    /// Command tabs: `lines` (the default) or `json`.
    pub(super) format: Option<String>,
    /// A tab whose rows programs publish over `rozi sidebar publish`.
    pub(super) stream: bool,
}

/// Build one launcher or command tab under an already-resolved id.
///
/// `Err` is a declaration that cannot become a tab at all; the caller decides what that means —
/// `config.toml` warns and skips it, an extension manifest treats it as an error that invalidates
/// the extension. Everything recoverable (a clamped interval, one unusable entry) goes to
/// `warnings` and the tab is still built.
pub(super) fn build_custom_tab(
    id: SidebarTabId,
    parts: CustomTabParts,
    warnings: &mut Vec<String>,
) -> Result<SidebarTab, String> {
    let name = id.as_str();
    let label = parts.label.trim();
    if label.is_empty() {
        return Err(format!("Sidebar tab `{name}` label must not be empty"));
    }
    if parts.stream {
        if parts.entries.is_some() || parts.command.is_some() {
            return Err(format!(
                "Sidebar tab `{name}` needs exactly one of `entries`, `command`, or `stream = true`"
            ));
        }
        for (field, present) in [
            ("interval", parts.interval.is_some()),
            ("on_click", parts.on_click.is_some()),
            ("group_prefix", parts.group_prefix.is_some()),
            ("format", parts.format.is_some()),
        ] {
            if present {
                warnings.push(format!(
                    "Sidebar tab `{name}` is a stream tab; ignoring `{field}` (its producers handle activation)"
                ));
            }
        }
        return Ok(SidebarTab::Stream {
            name: id.clone(),
            label: label.to_string(),
        });
    }
    match (parts.entries, parts.command) {
        (Some(entries), None) => {
            if parts.group_prefix.is_some() {
                warnings.push(format!(
                    "Sidebar tab `{name}` is a launcher; ignoring `group_prefix` (group its entries with `group` instead)"
                ));
            }
            if parts.format.is_some() {
                warnings.push(format!(
                    "Sidebar tab `{name}` is a launcher; ignoring `format`, which applies to command tabs"
                ));
            }
            let entries = entries
                .into_iter()
                .filter_map(|mut entry| {
                    let label = entry.label.trim().to_string();
                    if label.is_empty() {
                        warnings.push(format!(
                            "Sidebar tab `{name}` has an entry with an empty label; skipped"
                        ));
                        return None;
                    }
                    let group = entry
                        .group
                        .take()
                        .map(|group| group.trim().to_string())
                        .filter(|group| !group.is_empty());
                    let action = parse_sidebar_action(
                        entry.action(),
                        &format!("Sidebar entry `{label}` in `{name}`"),
                        "{line}",
                        warnings,
                    )?;
                    Some(SidebarLauncherEntry {
                        label,
                        group,
                        action,
                    })
                })
                .collect();
            Ok(SidebarTab::Launcher {
                name: id.clone(),
                label: label.to_string(),
                entries: cluster_by_group(entries),
                env: parts.env,
            })
        }
        (None, Some(command)) if !command.trim().is_empty() => {
            let requested = parts.interval.unwrap_or(30);
            let interval_secs = requested.max(SIDEBAR_MIN_COMMAND_INTERVAL_SECS);
            if interval_secs != requested {
                warnings.push(format!(
                    "Sidebar tab `{name}` interval {requested}s is below the minimum; clamped to {interval_secs}s"
                ));
            }
            let on_click = parts.on_click.and_then(|action| {
                parse_sidebar_action(
                    action,
                    &format!("Sidebar tab `{name}` on_click"),
                    "{line}",
                    warnings,
                )
            });
            // Not trimmed: a marker is often written with its trailing space (`"## "`), and that
            // space is part of what distinguishes it from an ordinary line. An empty one would turn
            // every row into a header, which is never meant.
            let group_prefix = parts.group_prefix.filter(|prefix| {
                if prefix.is_empty() {
                    warnings.push(format!(
                        "Sidebar tab `{name}` group_prefix must not be empty; ignored"
                    ));
                }
                !prefix.is_empty()
            });
            let format = match parts.format.as_deref() {
                None => super::schema::SidebarCommandFormat::Lines,
                Some(value) => super::schema::SidebarCommandFormat::parse(value).unwrap_or_else(|| {
                    warnings.push(format!(
                        "Sidebar tab `{name}` format `{value}` is unknown (expected `lines` or `json`); using `lines`"
                    ));
                    super::schema::SidebarCommandFormat::Lines
                }),
            };
            // A JSON command describes its own tree; a line marker has nothing to mark.
            let group_prefix = if format == super::schema::SidebarCommandFormat::Json
                && group_prefix.is_some()
            {
                warnings.push(format!(
                    "Sidebar tab `{name}` prints JSON; ignoring `group_prefix` (nest rows with `parent` instead)"
                ));
                None
            } else {
                group_prefix
            };
            Ok(SidebarTab::Command {
                name: id.clone(),
                label: label.to_string(),
                command: command.trim().to_string(),
                interval_secs,
                on_click,
                group_prefix,
                env: parts.env,
                format,
            })
        }
        _ => Err(format!(
            "Sidebar tab `{name}` needs exactly one of `entries`, `command`, or `stream = true`"
        )),
    }
}

fn build_tabs(raw: Vec<SidebarTabSpec>, warnings: &mut Vec<String>) -> Vec<SidebarTab> {
    let mut seen = HashSet::new();
    let mut tabs = Vec::new();
    for spec in raw {
        let tab = match spec {
            SidebarTabSpec::Name(name) => {
                match builtin_tab(name.trim().to_ascii_lowercase().as_str()) {
                    Some(tab) => tab,
                    None => {
                        warnings.push(format!("Unknown built-in sidebar tab `{name}`; skipped"));
                        continue;
                    }
                }
            }
            SidebarTabSpec::Table(table) => {
                let table = *table;
                let name = table.name.trim().to_string();
                if name.is_empty() {
                    warnings.push("Sidebar tab name must not be empty; skipped".to_string());
                    continue;
                }
                // A built-in tab is only ever named. Its settings have a table of their own, which
                // the settings picker can rewrite without touching the custom tabs listed here.
                if let Some(tab) = builtin_tab(&name) {
                    warnings.push(if builtin_tab_settings(&name).is_empty() {
                        format!("Sidebar tab `{name}` is built in; list it by name, not as a table")
                    } else {
                        format!(
                            "Sidebar tab `{name}` is built in; list it by name and set its options in `[sidebar.settings.{name}]`"
                        )
                    });
                    if seen.insert(tab.id()) {
                        tabs.push(tab);
                    }
                    continue;
                }
                let parts = CustomTabParts {
                    label: table.label,
                    entries: table.entries,
                    command: table.command,
                    interval: table.interval,
                    on_click: table.on_click,
                    group_prefix: table.group_prefix,
                    format: table.format,
                    stream: table.stream.unwrap_or(false),
                    env: Vec::new(),
                };
                match build_custom_tab(SidebarTabId::new(name), parts, warnings) {
                    Ok(tab) => tab,
                    Err(error) => {
                        warnings.push(format!("{error}; skipped"));
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
        parse_with_extensions(text, Vec::new())
    }

    fn parse_with_extensions(
        text: &str,
        extension_tabs: Vec<SidebarTab>,
    ) -> (SidebarConfig, Vec<String>) {
        let raw = toml::from_str(text).expect("sidebar config parses");
        let mut config = SidebarConfig::default();
        let mut warnings = Vec::new();
        apply_sidebar_config(
            &mut config,
            raw,
            SidebarContributions {
                tabs: extension_tabs,
                ..Default::default()
            },
            &mut warnings,
        );
        (config, warnings)
    }

    fn extension_tab(id: &str) -> SidebarTab {
        SidebarTab::Launcher {
            name: SidebarTabId::new(id),
            label: "Agents".to_string(),
            env: Vec::new(),
            entries: vec![SidebarLauncherEntry {
                label: "rozi".to_string(),
                group: None,
                action: crate::config::UserCommandAction::run("claude"),
            }],
        }
    }

    /// `format = "json"` makes a command tab print a tree; `stream = true` makes a tab that only
    /// producers fill, and it takes neither a command nor entries.
    #[test]
    fn command_tabs_take_a_format_and_stream_tabs_stand_alone() {
        let (config, warnings) = parse(
            r#"tabs = [
              { name = "deps", label = "Deps", command = "deps --json", format = "json", group_prefix = "> " },
              { name = "deploys", label = "Deploys", stream = true },
              { name = "both", label = "Both", stream = true, command = "x" },
              { name = "odd", label = "Odd", command = "x", format = "yaml" },
            ]"#,
        );
        assert!(matches!(
            &config.tabs[0],
            SidebarTab::Command {
                format: crate::config::SidebarCommandFormat::Json,
                group_prefix: None,
                ..
            }
        ));
        assert!(matches!(&config.tabs[1], SidebarTab::Stream { .. }));
        assert_eq!(
            config.tabs.len(),
            3,
            "a stream tab with a command is refused"
        );
        assert!(matches!(
            &config.tabs[2],
            SidebarTab::Command {
                format: crate::config::SidebarCommandFormat::Lines,
                ..
            }
        ));
        for expected in [
            "ignoring `group_prefix`",
            "`stream = true`",
            "format `yaml` is unknown",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(expected)),
                "{expected}: {warnings:?}"
            );
        }
    }

    /// `[sidebar.settings.activity]` sets how worktrees of one repository are laid out, and warns
    /// about anything it cannot honour rather than guessing.
    #[test]
    fn the_activity_tab_takes_a_worktree_layout() {
        let (config, warnings) =
            parse("tabs = [\"activity\"]\n[settings.activity]\nworktrees = \"repository\"\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            config.tabs,
            vec![SidebarTab::Activity {
                config: crate::config::SidebarActivityConfig {
                    worktrees: crate::config::ActivityWorktrees::Repository,
                },
            }]
        );
        let group = &config.tab_settings["activity"];
        assert_eq!(
            group.owner,
            crate::config::SettingsOwner::SidebarTab("activity".into())
        );
        assert!(group.overridden.contains("worktrees"));

        // The default tab list still reads its settings.
        let (config, _) = parse("[settings.activity]\nworktrees = \"repository\"\n");
        assert!(matches!(
            config.tabs[0],
            SidebarTab::Activity {
                config: crate::config::SidebarActivityConfig {
                    worktrees: crate::config::ActivityWorktrees::Repository,
                },
            }
        ));

        let (config, warnings) = parse(
            "tabs = [\"activity\", \"activity\"]\n[settings.activity]\nworktrees = \"nested\"\nroot = \"repo\"\n",
        );
        assert_eq!(
            config.tabs,
            vec![SidebarTab::activity()],
            "the duplicate is skipped"
        );
        for expected in [
            "`nested` is unknown",
            "has no setting `root`",
            "Duplicate sidebar tab",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(expected)),
                "{expected}: {warnings:?}"
            );
        }
    }

    /// A built-in tab is only named in `tabs`. A table for one keeps the tab where it is, ignores
    /// its options, and says where they belong.
    #[test]
    fn a_builtin_tab_table_points_at_its_settings_table() {
        let (config, warnings) = parse(
            r#"tabs = [{ name = "files", explorer = true }, { name = "panes", label = "Mine", command = "ls" }]"#,
        );
        assert_eq!(
            config.tabs.iter().map(SidebarTab::id).collect::<Vec<_>>(),
            [SidebarTabId::new("files"), SidebarTabId::new("panes")]
        );
        assert!(!tree(&config, "files").explorer);
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("[sidebar.settings.files]")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("not as a table")),
            "{warnings:?}"
        );
    }

    #[test]
    fn settings_tables_for_other_tabs_are_reported() {
        let (config, warnings) = parse(
            "[settings.panes]\nx = 1\n[settings.\"tasks.jobs\"]\ny = 1\n[settings.nope]\nz = 1\n",
        );
        assert!(!config.tab_settings.contains_key("panes"));
        for expected in [
            "has no settings",
            "[extensions.tasks]",
            "names no built-in tab",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(expected)),
                "{expected}: {warnings:?}"
            );
        }
    }

    /// Every configurable built-in has a group even when nothing is set, so the picker always has
    /// its settings to show; tabs with none have no group.
    #[test]
    fn builtin_groups_exist_at_their_defaults() {
        let config = SidebarConfig::default();
        let keys: Vec<_> = config.tab_settings.keys().map(String::as_str).collect();
        assert_eq!(keys, ["activity", "files", "git", "worktrees"]);
        let git = &config.tab_settings["git"];
        assert!(git.overridden.is_empty());
        assert_eq!(git.values["diff_stats"], SettingValue::Bool(true));
        assert!(git.spec("compact_dirs").is_some());
        assert!(config.tab_settings["files"].spec("compact_dirs").is_none());
    }

    /// A layout that keeps every tab on the right gets new tabs on the right too, rather than a left
    /// dock it never had.
    #[test]
    fn unplaced_tabs_join_the_dock_in_use() {
        let (config, _) = parse(
            r#"layout = { right = { panel_count = 2, panels = [
              { weight = 0.5, tabs = ["activity", "panes", "sessions"] },
              { weight = 0.5, tabs = ["files", "git", "worktrees"] },
            ] }, left = { panel_count = 1, panels = [{ weight = 1.0, tabs = [] }] } }"#,
        );
        assert_eq!(
            config.layout.location("agents"),
            Some((SidebarPosition::Right, 0))
        );
        assert_eq!(
            config.layout.location("projects"),
            Some((SidebarPosition::Right, 1))
        );
        let (config, _) = parse("startup = true\n");
        assert_eq!(
            config.layout.location("agents"),
            Some((SidebarPosition::Left, 0))
        );
    }

    #[test]
    fn sidebar_position_round_trips_and_toggles() {
        for position in [SidebarPosition::Left, SidebarPosition::Right] {
            assert_eq!(SidebarPosition::parse(position.id()), Some(position));
            assert_eq!(position.toggled().toggled(), position);
        }
        assert_eq!(SidebarPosition::Left.label(), "Left");
        assert_eq!(SidebarPosition::Right.label(), "Right");
    }

    #[test]
    fn appearance_flags_apply_without_replacing_the_tab_catalog() {
        let (config, warnings) = parse(
            "background_follows_canvas = true\ngap = false\nbackground = false\ntab_style = \"round\"\n",
        );
        assert!(
            warnings.iter().all(|w| w.starts_with("Legacy sidebar")),
            "{warnings:?}"
        );
        assert!(config.background_follows_canvas);
        assert!(!config.gap);
        assert!(!config.background);
        assert_eq!(config.tab_style, tui_lipan::prelude::CapStyle::Round);
        assert_eq!(config.layout, SidebarConfig::default().layout);
    }

    #[test]
    fn half_tab_style_is_ignored_with_a_warning() {
        let (config, warnings) = parse("tab_style = \"half\"\n");
        assert_eq!(config.tab_style, tui_lipan::prelude::CapStyle::Padded);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("half block")),
            "{warnings:?}"
        );
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
        assert!(
            warnings.iter().all(|w| w.starts_with("Legacy sidebar")),
            "{warnings:?}"
        );
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
        assert!(files.show_hidden);
        assert!(!files.diff_stats);
        let git = tree(&config, "git");
        assert_eq!(git.root, SidebarTreeRoot::Repo);
        assert!(git.show_hidden);
        assert!(git.diff_stats);
        assert!(git.compact_dirs);
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
    fn file_tree_settings_override_options() {
        let (config, warnings) = parse(
            r#"
            tabs = ["files", "git"]
            [settings.files]
            show_hidden = false
            explorer = true
            diff_stats = true
            max_entries = 50
            root = "repo"
            on_click = { send = "nvim {path}\n" }
            [settings.git]
            compact_dirs = false
            "#,
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        let files = tree(&config, "files");
        assert_eq!(files.root, SidebarTreeRoot::Repo);
        assert!(!files.show_hidden);
        assert!(files.explorer && files.diff_stats);
        assert_eq!(files.max_entries, 50);
        assert_eq!(
            files.on_click,
            Some(super::super::schema::UserCommandAction::Send(
                "nvim {path}\n".into()
            ))
        );
        assert!(!tree(&config, "git").compact_dirs);
        // `on_click` is configuration, not a value the picker edits.
        assert!(config.tab_settings["files"].spec("on_click").is_none());
        assert!(!config.tab_settings["files"].overridden.contains("on_click"));
    }

    /// `{path}` may only reach the pane as literal keystrokes. A `run`/`popup` command line is
    /// executed, and a filename in the repository must never be able to compose one.
    #[test]
    fn file_tree_rejects_path_placeholder_in_executed_actions() {
        let (config, warnings) = parse("[settings.files]\non_click = { run = \"rm {path}\" }\n");
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("{path}"), "{warnings:?}");
        assert_eq!(tree(&config, "files").on_click, None);

        // The supported way to scope a command to the clicked file: the path is referenced through
        // the environment, so nothing is spliced into the command line and it is accepted.
        let (config, warnings) =
            parse("[settings.git]\non_click = { run = \"lazygit -f \\\"$ROZI_FILE\\\"\" }\n");
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            tree(&config, "git").on_click,
            Some(super::super::schema::UserCommandAction::run(
                "lazygit -f \"$ROZI_FILE\""
            ))
        );
    }

    #[test]
    fn file_tree_settings_are_validated_and_clamped() {
        let (config, warnings) = parse(
            "[settings.files]\nroot = \"elsewhere\"\nmax_entries = 0\ncompact_dirs = false\nexplorer = \"yes\"\n",
        );
        assert_eq!(tree(&config, "files").root, SidebarTreeRoot::Cwd);
        assert_eq!(tree(&config, "files").max_entries, 1);
        assert!(!tree(&config, "files").explorer);
        for expected in [
            "`elsewhere` is unknown",
            "max_entries` 0 is out of range; clamped to 1",
            // Compact directories only apply to the changes view.
            "has no setting `compact_dirs`",
            "is string, but `explorer` is boolean",
        ] {
            assert!(
                warnings.iter().any(|warning| warning.contains(expected)),
                "{expected}: {warnings:?}"
            );
        }
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
        assert!(
            warnings.iter().all(|w| w.starts_with("Legacy sidebar")),
            "{warnings:?}"
        );
        assert!(config.startup);
        assert_eq!(config.migrated_docks, Some([false, true]));
        assert_eq!(config.layout.right.panel_count, 2);
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
        assert_eq!(
            config.layout.left.width,
            super::super::schema::SIDEBAR_MIN_WIDTH
        );
        assert_eq!(config.tabs.len(), 2);
        assert!(
            matches!(&config.tabs[1], SidebarTab::Launcher { entries, .. } if entries.len() == 1)
        );
        assert!(warnings.len() >= 5, "{warnings:?}");
    }

    #[test]
    fn unknown_table_fields_are_ignored() {
        let (config, warnings) =
            parse(r#"tabs = [{ name = "x", label = "X", entries = [], typo = true }]"#);
        assert!(warnings.is_empty());
        assert_eq!(config.tabs.len(), 1);
        assert_eq!(config.tabs[0].id().as_str(), "x");
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

    /// An extension tab has to become reachable without rebuilding a placement the user never asked
    /// to change: appending it must not collapse the default two panels into one.
    /// The user's own arrangement wins: a `panels` entry naming the extension tab keeps it where it
    /// was dragged rather than sending it back to the first panel.
    #[test]
    fn a_config_tab_of_the_same_id_wins_over_the_extension_one() {
        let (config, warnings) = parse_with_extensions(
            r#"tabs = ["panes", { name = "git-tools.agents", label = "Mine", entries = [{ label = "Date", run = "date" }] }]"#,
            vec![extension_tab("git-tools.agents")],
        );
        assert_eq!(config.tabs.len(), 2);
        assert_eq!(config.tabs[1].label(), "Mine");
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("already configured")),
            "{warnings:?}"
        );
    }

    /// A placement naming an extension tab that is not loaded right now is data the user chose, not
    /// a typo: it is skipped in silence and restored when the extension returns. A bare name that
    /// resolves to nothing is still a typo and still says so.
    #[test]
    fn launcher_entries_cluster_under_their_groups_with_ungrouped_ones_leading() {
        let (config, warnings) = parse(
            r#"
            tabs = [{ name = "launch", label = "Launch", entries = [
              { label = "Claude here", group = "claude", run = "claude" },
              { label = "Loose", run = "date" },
              { label = "Codex here", group = " codex ", run = "codex" },
              { label = "Claude in rozi", group = "claude", run = "claude" },
              { label = "Blank group", group = "  ", run = "date" },
            ] }]
        "#,
        );
        assert!(
            warnings.iter().all(|w| w.starts_with("Legacy sidebar")),
            "{warnings:?}"
        );
        let SidebarTab::Launcher { entries, .. } = &config.tabs[0] else {
            panic!("launcher tab");
        };
        // Ungrouped first in config order, then each group in first-appearance order with its own
        // entries kept in config order. A whitespace-only group is no group at all.
        assert_eq!(
            entries
                .iter()
                .map(|entry| (entry.label.as_str(), entry.group.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("Loose", None),
                ("Blank group", None),
                ("Claude here", Some("claude")),
                ("Claude in rozi", Some("claude")),
                ("Codex here", Some("codex")),
            ]
        );
    }

    #[test]
    fn group_prefix_belongs_to_command_tabs_and_must_not_be_empty() {
        let (config, warnings) = parse(
            r#"
            tabs = [
              { name = "todos", label = "Todos", command = "task list", group_prefix = "-- " },
              { name = "empty", label = "Empty", command = "task list", group_prefix = "" },
              { name = "launch", label = "Launch", group_prefix = "-- ", entries = [
                { label = "Date", run = "date" },
              ] },
            ]
        "#,
        );
        assert!(matches!(
            &config.tabs[0],
            SidebarTab::Command { group_prefix: Some(prefix), .. } if prefix == "-- "
        ));
        assert!(matches!(
            &config.tabs[1],
            SidebarTab::Command {
                group_prefix: None,
                ..
            }
        ));
        assert!(matches!(&config.tabs[2], SidebarTab::Launcher { .. }));
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("must not be empty")));
        assert!(warnings.iter().any(|w| w.contains("is a launcher")));
    }
}

#[cfg(test)]
mod docking_contract_tests {
    use super::super::schema::*;

    #[test]
    fn compacting_is_lossless_and_transfer_is_unique() {
        let mut layout = SidebarDockLayout::default();
        let original = layout.left.panels.clone();
        layout.left.panel_count = 1;
        layout.validate(&mut Vec::new());
        assert_eq!(layout.left.panels, original);
        layout.place("files", SidebarPosition::Right, 2);
        assert_eq!(layout.location("files"), Some((SidebarPosition::Right, 2)));
        assert!(
            !layout
                .left
                .panels
                .iter()
                .any(|p| p.tabs.contains(&"files".into()))
        );
        layout.hidden.push("files".into());
        layout.left.panel_count = 3;
        layout.validate(&mut Vec::new());
        assert_eq!(layout.location("files"), Some((SidebarPosition::Right, 2)));
        assert!(layout.hidden.contains(&"files".into()));
    }

    #[test]
    fn validation_retains_unknown_tabs_and_dormant_panels() {
        let mut layout = SidebarDockLayout::default();
        layout.place("removed.extension", SidebarPosition::Right, 4);
        layout.right.panels[4].weight = f32::NAN;
        layout.right.panels[0].tabs.push("files".into());
        let mut warnings = Vec::new();
        layout.validate(&mut warnings);
        assert_eq!(layout.right.panels.len(), 5);
        assert_eq!(layout.right.panels[4].weight, 1.0);
        assert_eq!(
            layout.location("removed.extension"),
            Some((SidebarPosition::Right, 4))
        );
        assert_eq!(warnings.len(), 2);
    }
}

#[cfg(test)]
mod layout_loading_tests {
    use super::*;

    fn load(
        text: &str,
        tabs: Vec<SidebarTab>,
        suggestions: std::collections::BTreeMap<String, super::super::schema::SidebarTabLocation>,
    ) -> (SidebarConfig, Vec<String>) {
        let mut config = SidebarConfig::default();
        let mut warnings = Vec::new();
        apply_sidebar_config(
            &mut config,
            toml::from_str(text).unwrap(),
            SidebarContributions {
                tabs,
                suggestions,
                ..Default::default()
            },
            &mut warnings,
        );
        (config, warnings)
    }

    #[test]
    fn old_dock_startup_flags_migrate_with_explicit_startup_taking_precedence() {
        for docks in [[false, false], [true, false], [false, true], [true, true]] {
            let text = format!(
                "layout = {{ left = {{ visible = {} }}, right = {{ visible = {} }} }}",
                docks[0], docks[1]
            );
            let (config, warnings) = load(&text, Vec::new(), Default::default());
            assert_eq!(config.startup, docks.iter().any(|shown| *shown));
            assert_eq!(config.migrated_docks, config.startup.then_some(docks));
            assert_eq!(warnings.len(), 1);
            assert!(warnings[0].contains("migrated to sidebar.startup"));
            let (explicit, _) = load(
                &format!("startup = false\n{text}"),
                Vec::new(),
                Default::default(),
            );
            assert!(!explicit.startup);
            assert_eq!(explicit.migrated_docks, config.migrated_docks);
        }
    }

    #[test]
    fn legacy_layout_migrates_with_an_explicit_diagnostic() {
        let (config, warnings) = load(
            r#"visible = true
position = "right"
width = 41
panels = [["panes", "gone.tab"], ["files"]]
split = false
split_ratio = 0.7"#,
            Vec::new(),
            Default::default(),
        );
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("Legacy sidebar"));
        assert!(config.startup);
        assert_eq!(config.migrated_docks, Some([false, true]));
        assert_eq!(config.layout.right.width, 41);
        assert_eq!(config.layout.right.panel_count, 1);
        assert_eq!(config.layout.right.panels[0].weight, 0.7);
        assert_eq!(
            config.layout.location("gone.tab"),
            Some((SidebarPosition::Right, 0))
        );
    }

    #[test]
    fn explicit_placement_beats_extension_suggestion_and_absence_is_lossless() {
        let id = SidebarTabId::new("tools.jobs");
        let tab = SidebarTab::Launcher {
            name: id,
            label: "Jobs".into(),
            entries: Vec::new(),
            env: Vec::new(),
        };
        let suggestions = [(
            "tools.jobs".into(),
            super::super::schema::SidebarTabLocation {
                dock: "right".into(),
                panel: 3,
            },
        )]
        .into_iter()
        .collect();
        let (suggested, _) = load("", vec![tab.clone()], suggestions);
        assert_eq!(
            suggested.layout.location("tools.jobs"),
            Some((SidebarPosition::Right, 2))
        );
        let text = r#"layout = { hidden = ["tools.jobs"], left = { panels = [{ tabs = ["tools.jobs"] }] } }"#;
        let (absent, warnings) = load(text, Vec::new(), Default::default());
        assert!(warnings.is_empty());
        let (restored, _) = load(text, vec![tab], Default::default());
        assert_eq!(
            absent.layout.location("tools.jobs"),
            restored.layout.location("tools.jobs")
        );
        assert_eq!(
            restored.layout.location("tools.jobs"),
            Some((SidebarPosition::Left, 0))
        );
        assert!(restored.layout.hidden.contains(&"tools.jobs".into()));
    }
}
