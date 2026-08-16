#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum HelpKind {
    Global,
    Direct,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct HelpRow {
    category: String,
    keys: String,
    label: String,
    kind: HelpKind,
    extra: String,
}

impl HelpRow {
    fn global(category: &str, keys: &str, label: &str) -> Self {
        Self {
            category: category.to_string(),
            keys: keys.to_string(),
            label: label.to_string(),
            kind: HelpKind::Global,
            extra: String::new(),
        }
    }

    fn unbound(category: &str, label: &str) -> Self {
        Self {
            extra: "unbound".to_string(),
            ..Self::global(category, "", label)
        }
    }

    fn direct(category: &str, keys: &str, label: &str, extra: &str) -> Self {
        Self {
            category: category.to_string(),
            keys: keys.to_string(),
            label: label.to_string(),
            kind: HelpKind::Direct,
            extra: extra.to_string(),
        }
    }
}

fn help_rows(ctx: &Context<AppRoot>) -> Vec<HelpRow> {
    let mut rows = ctx
        .command_registry()
        .entries()
        .into_iter()
        .filter(|entry| {
            entry.id.as_str() != crate::commands::FORWARD_PREFIX_COMMAND_ID
                && entry.id.as_str() != "detach"
                && !entry.id.as_str().starts_with("app.")
                && !entry.id.as_str().starts_with("workspace.")
        })
        .map(|entry| {
            let category = entry.category.as_deref().unwrap_or("Other");
            let keys = entry.keybinding_hint.as_deref().unwrap_or("");
            let label = entry.label.as_ref();
            if keys.is_empty() {
                HelpRow::unbound(category, label)
            } else {
                HelpRow::global(category, keys, label)
            }
        })
        .collect::<Vec<_>>();
    rows.splice(0..0, scheme_rows(&ctx.state.config.input));
    rows.extend([
        HelpRow::global("Workspaces", "1-9", "Switch to workspace"),
        HelpRow::global("Workspaces", "Shift+1-9", "Move pane to workspace (follow)"),
        HelpRow::global(
            "Workspaces",
            "Ctrl+Shift+1-9",
            "Move workspace to workspace (follow)",
        ),
        HelpRow::global("Mouse", "drag", "Move pane (left-drag)"),
        HelpRow::global("Mouse", "right-drag", "Resize pane from corner"),
        HelpRow::global("Mouse", "drag gap", "Resize a tiled split"),
    ]);
    rows.extend(direct_mode_rows());
    rows
}

fn direct_mode_rows() -> Vec<HelpRow> {
    const COPY: &str = "Copy mode · DIRECT";
    const SIDEBAR: &str = "Sidebar focused · DIRECT";
    const COPY_EXTRA: &str = "direct copy mode context selection";
    const SIDEBAR_EXTRA: &str = "direct sidebar focused context tree files git panel";
    let mut rows = vec![
        HelpRow::direct(COPY, "hjkl / arrows", "Move cursor", COPY_EXTRA),
        HelpRow::direct(COPY, "w / b / e", "Word forward / back / end", COPY_EXTRA),
        HelpRow::direct(COPY, "W / B / E", "WORD forward / back / end", COPY_EXTRA),
        HelpRow::direct(
            COPY,
            "0 / ^ / $",
            "Line start / first non-blank / end",
            COPY_EXTRA,
        ),
        HelpRow::direct(COPY, "g / G", "Top / bottom of scrollback", COPY_EXTRA),
        HelpRow::direct(COPY, "Ctrl+u / Ctrl+d", "Half page up / down", COPY_EXTRA),
        HelpRow::direct(COPY, "v / Space", "Start selection", COPY_EXTRA),
        HelpRow::direct(COPY, "y / Enter", "Copy selection & exit", COPY_EXTRA),
        HelpRow::direct(COPY, "Esc / q", "Exit copy mode", COPY_EXTRA),
        HelpRow::direct(SIDEBAR, "↑ / k, ↓ / j", "Move cursor", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "PageUp / PageDown", "Move one page", SIDEBAR_EXTRA),
        HelpRow::direct(
            SIDEBAR,
            "Home / g, End / G",
            "First / last row",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(SIDEBAR, "Enter", "Activate", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "Tab / Shift+Tab", "Cycle tabs", SIDEBAR_EXTRA),
        HelpRow::direct(
            SIDEBAR,
            "← / h, → / l, Space",
            "Collapse / expand / toggle dirs",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+Shift+← / Ctrl+Shift+→",
            "Reorder the active tab",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+↑ / Ctrl+↓",
            "Focus the other panel",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Ctrl+Shift+↑ / Ctrl+Shift+↓",
            "Move tab to the other panel",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Shift+← / Shift+→",
            "Resize sidebar",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(
            SIDEBAR,
            "Shift+↑ / Shift+↓",
            "Resize panel split",
            SIDEBAR_EXTRA,
        ),
        HelpRow::direct(SIDEBAR, "s", "Toggle panels", SIDEBAR_EXTRA),
        HelpRow::direct(SIDEBAR, "Esc", "Return to pane", SIDEBAR_EXTRA),
    ];
    for row in &mut rows {
        row.extra.push_str(" modes");
    }
    rows
}

fn filtered_help_groups(
    rows: impl IntoIterator<Item = HelpRow>,
    tab: crate::state::HelpTab,
    query: &str,
) -> Vec<(String, Vec<HelpRow>)> {
    let query = normalize_help_query(query);
    let mut groups: Vec<(String, Vec<HelpRow>)> = Vec::new();
    for row in rows {
        let include = match tab {
            crate::state::HelpTab::Global => row.kind == HelpKind::Global && !row.keys.is_empty(),
            crate::state::HelpTab::Modes => row.kind == HelpKind::Direct,
            crate::state::HelpTab::Unbound => row.kind == HelpKind::Global && row.keys.is_empty(),
            crate::state::HelpTab::All => true,
        };
        if !include {
            continue;
        }
        if !query.is_empty() && !help_row_matches(&row, &query) {
            continue;
        }
        match groups
            .iter_mut()
            .find(|(category, _)| *category == row.category)
        {
            Some((_, entries)) => entries.push(row),
            None => groups.push((row.category.clone(), vec![row])),
        }
    }
    groups.sort_by_key(|(category, _)| help_category_priority(category));
    groups
}

fn normalize_help_query(query: &str) -> String {
    collapse_ws(
        &query
            .trim()
            .to_ascii_lowercase()
            .replace(['+', '/', ',', '·'], " "),
    )
}

fn help_row_matches(row: &HelpRow, query: &str) -> bool {
    help_row_haystack(row).contains(query)
}

fn help_row_haystack(row: &HelpRow) -> String {
    collapse_ws(&format!(
        "{} {} {} {}",
        row.category
            .to_ascii_lowercase()
            .replace(['·', '+', '/'], " "),
        normalize_help_keys(&row.keys),
        row.label.to_ascii_lowercase(),
        row.extra.to_ascii_lowercase()
    ))
}

fn normalize_help_keys(keys: &str) -> String {
    keys.to_ascii_lowercase()
        .replace('←', " left ")
        .replace('→', " right ")
        .replace('↑', " up ")
        .replace('↓', " down ")
        .replace(['+', '/', ',', '·'], " ")
}

fn collapse_ws(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn scheme_rows(input: &crate::config::InputConfig) -> Vec<HelpRow> {
    let prefix = crate::keys_display::format_binding(&input.prefix);
    let mut rows = vec![HelpRow {
        extra: "prefix then key scheme".to_string(),
        ..HelpRow::global("", &prefix, "Prefix · then key")
    }];
    if input.modifier_shortcuts {
        rows.push(HelpRow {
            extra: "mod hold key scheme".to_string(),
            ..HelpRow::global("", input.modifier.label(), "Mod · hold + key")
        });
    }
    rows
}

pub(crate) fn help_overlay(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let filter = Input::bound(&ctx.state.help_query)
        .placeholder("Search… (/)")
        .style(fg_only(&theme.muted))
        .focus_style(Style::new().fg(theme.border_active))
        .placeholder_style(fg_only(&theme.muted))
        .selection_style(theme.text_selection)
        .width(Length::Auto)
        .height(Length::Px(1))
        .border(false)
        .padding(0)
        .tab_stop(false)
        .on_change(ctx.link().callback(Msg::HelpQueryChanged))
        .key_interceptor(ctx.link().key_handler(|key| {
            if key.code == KeyCode::Enter && !key.mods.ctrl && !key.mods.alt && !key.mods.super_key
            {
                return Some(Msg::HelpBlurFilter);
            }
            key.is(KeyCode::Esc).then_some(Msg::HelpEscape)
        }))
        .key(help_filter_key());
    let header = HStack::new()
        .justify(Justify::End)
        .height(Length::Px(1))
        .child(filter.min_width(Length::Px(11)).max_width(Length::Px(22)));
    let caps = ctx
        .state
        .config
        .pane
        .workbar_tab_style
        .glyphs()
        .and_then(|(left, right)| Some((left.chars().next()?, right.chars().next()?)));
    let tabs = Tabs::new()
        .tabs(vec![
            Tab::new("Global"),
            Tab::new("Modes"),
            Tab::new("Unbound"),
            Tab::new("All"),
        ])
        .active(match ctx.state.help_tab {
            crate::state::HelpTab::Global => 0,
            crate::state::HelpTab::Modes => 1,
            crate::state::HelpTab::Unbound => 2,
            crate::state::HelpTab::All => 3,
        })
        .focusable(false)
        .width(Length::Flex(1))
        .height(Length::Px(1))
        .divider(' ')
        .caps(caps)
        .style(Style::new().fg(theme.surface.menu).bg(theme.surface.panel))
        .active_style(
            Style::new()
                .fg(theme.surface.backdrop)
                .bg(theme.border_active)
                .bold(),
        )
        .tab_hover_style(Style::new().transform_bg(crate::view::hover_lift()))
        .on_change(
            ctx.link()
                .callback(|event: TabsEvent| Msg::HelpTabSelected(event.index)),
        );
    let groups = filtered_help_groups(
        help_rows(ctx),
        ctx.state.help_tab,
        ctx.state.help_query.text(),
    );
    let mut list = VStack::new();
    if groups.is_empty() {
        list = list.child(
            Text::new("No matches")
                .style(fg_only(&theme.muted))
                .height(Length::Px(1)),
        );
    } else {
        for (index, (category, rows)) in groups.iter().enumerate() {
            if !category.is_empty() {
                list = list.child(help_section(category, theme, index > 0));
            }
            for row in rows {
                list = list.child(help_row(&row.keys, &row.label, theme));
            }
        }
    }
    let body = VStack::new()
        .child(tabs)
        .child(Text::new("").height(Length::Px(1)))
        .child(
            ScrollView::new()
                .children(vec![list.into()])
                .focusable(true)
                .scroll_wheel(true)
                .scrollbar(true)
                .scrollbar_config(modal_scrollbar_config(theme))
                .height(Length::Flex(1))
                .key(help_scroll_key()),
        );
    Modal::new()
        .width(Length::Px(64))
        .height(Length::Percent(70))
        .padding(0)
        .border(false)
        .frame_style(Style::new().bg(theme.surface.element))
        .dismiss_on_escape(false)
        .on_close(ctx.link().callback(|_| Msg::CloseHelp))
        .child(
            Frame::new()
                .header_left("Keybindings")
                .header_style(theme.accent.bold())
                .header_content(header)
                .border(true)
                .border_style(BorderStyle::Rounded)
                .style(Style::new().bg(theme.surface.element))
                .padding((0, 1, 1, 1))
                .height(Length::Flex(1))
                .child(body),
        )
        .into()
}

fn help_category_priority(category: &str) -> usize {
    match category {
        "" => 0,
        "App" => 1,
        "Session" => 2,
        "Collaboration" => 3,
        "Panes" => 4,
        "Focus" => 5,
        "Workspace" => 6,
        "Workspaces" => 7,
        "Copy mode · DIRECT" => 8,
        "Profile" => 9,
        "Settings" => 10,
        "Mouse" => 11,
        "Sidebar" => 12,
        "Sidebar focused · DIRECT" => 13,
        "Custom" => usize::MAX,
        _ => 14,
    }
}

fn help_section(title: &str, theme: &Theme, spaced: bool) -> Element {
    let divider = Divider::horizontal()
        .label(
            Text::new(format!(" {} ", title.to_uppercase())).style(fg_only(&theme.accent).bold()),
        )
        .label_alignment(Align::Center)
        .label_padding(1)
        .style(fg_only(&theme.muted))
        .width(Length::Flex(1));
    if spaced {
        VStack::new()
            .child(Text::new("").height(Length::Px(1)))
            .child(divider)
            .into()
    } else {
        divider.into()
    }
}

fn help_row(keys: &str, desc: &str, theme: &Theme) -> Element {
    let (keys_text, keys_style) = if keys.is_empty() {
        ("—".to_string(), fg_only(&theme.muted))
    } else {
        (
            crate::keys_display::format_keys(keys),
            Style::new().fg(theme.border_active).bold(),
        )
    };
    HStack::new()
        .gap(2)
        .height(Length::Px(1))
        .justify(SpaceBetween)
        .child(
            Text::new(keys_text)
                .style(keys_style)
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .child(
            Text::new(desc.to_string())
                .style(fg_only(&theme.primary))
                .width(Length::Auto)
                .height(Length::Px(1))
                .overflow(Overflow::Ellipsis),
        )
        .into()
}

#[cfg(test)]
mod palette_alias_tests {
    use super::{
        HelpRow, command_entries_with_groups, command_palette_aliases, filtered_help_groups,
        help_category_priority, scheme_rows, settings_palette_aliases,
    };
    use crate::state::{HelpTab, SettingsAction};
    use tui_lipan::prelude::SearchEntry;

    /// `settings_palette_aliases` always appends the group name, so a row with no aliases of its
    /// own still returns a one-element list. Assert on the aliases *besides* the group, or the
    /// check passes for every row whether or not anyone wrote one.
    #[test]
    fn every_settings_action_has_search_aliases() {
        for action in SettingsAction::all().iter().copied() {
            let group = "Test group";
            let aliases = settings_palette_aliases(group, action);
            let own: Vec<_> = aliases
                .iter()
                .filter(|alias| alias.as_ref() != group)
                .collect();
            assert!(
                !own.is_empty(),
                "{action:?} is searchable only by its group name; give it aliases of its own"
            );
        }
    }

    /// Every row must also be reachable from the Commands palette, where `Settings` is the only
    /// entry standing in for all of them - typing what you want to change has to find the door.
    #[test]
    fn settings_command_aliases_cover_every_group() {
        let aliases = command_palette_aliases("settings");
        for group in [
            "theme",
            "titlebar",
            "workbar",
            "which key",
            "alerts",
            "notifications",
            "sounds",
            "session startup",
        ] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == group),
                "the Settings command is unreachable by `{group}`"
            );
        }
    }

    #[test]
    fn settings_command_aliases_cover_nested_controls() {
        let aliases = command_palette_aliases("settings");
        for term in [
            "padding",
            "powerline",
            "workbar badge",
            "workbar tab",
            "workbar style",
            "titlebar style",
            "focused titlebar",
        ] {
            assert!(
                aliases.iter().any(|alias| alias.as_ref() == term),
                "missing settings alias: {term}"
            );
        }
    }

    #[test]
    fn toggle_sidebar_has_sidebar_alias() {
        let aliases = command_palette_aliases("toggle-sidebar");
        assert!(
            aliases.iter().any(|alias| alias.as_ref() == "sidebar"),
            "toggle-sidebar must keep an exact sidebar alias for Hybrid ranking"
        );
        for id in [
            "toggle-sidebar-split",
            "focus-sidebar",
            "sidebar-next-tab",
            "sidebar-prev-tab",
        ] {
            assert!(
                command_palette_aliases(id).is_empty(),
                "{id} needs no redundant sidebar alias"
            );
        }
    }

    #[test]
    fn command_groups_have_exactly_one_spacer_between_them() {
        let entries = command_entries_with_groups([
            ("Panes", vec![SearchEntry::item("Pane", 1)]),
            ("Workspace", vec![SearchEntry::item("Workspace", 2)]),
            ("App", vec![SearchEntry::item("App", 3)]),
        ]);
        assert!(matches!(entries[0], SearchEntry::Header(_)));
        assert!(matches!(entries[1], SearchEntry::Item(_)));
        assert!(matches!(entries[2], SearchEntry::Spacer));
        assert!(matches!(entries[3], SearchEntry::Header(_)));
        assert!(matches!(entries[4], SearchEntry::Item(_)));
        assert!(matches!(entries[5], SearchEntry::Spacer));
        assert!(matches!(entries[6], SearchEntry::Header(_)));
        assert!(matches!(entries[7], SearchEntry::Item(_)));
        assert_eq!(
            entries
                .iter()
                .filter(|entry| matches!(entry, SearchEntry::Spacer))
                .count(),
            2
        );
    }

    #[test]
    fn help_categories_put_custom_last() {
        let categories = [
            "Settings",
            "Workspace",
            "Session",
            "Collaboration",
            "App",
            "Panes",
            "Profile",
            "Sidebar",
            "Custom",
            "Other",
        ];
        let mut sorted = categories;
        sorted.sort_by_key(|category| help_category_priority(category));
        assert_eq!(
            sorted,
            [
                "App",
                "Session",
                "Collaboration",
                "Panes",
                "Workspace",
                "Profile",
                "Settings",
                "Sidebar",
                "Other",
                "Custom",
            ]
        );
    }

    #[test]
    fn unbound_tab_keeps_empty_keys_out_of_global() {
        let rows = vec![
            HelpRow::unbound("App", "Edit scrollback"),
            HelpRow::unbound("App", "Open config file"),
            HelpRow::unbound("Workspace", "Kill workspace"),
            HelpRow::global("Panes", "Enter", "New pane"),
            HelpRow::direct("Copy mode · DIRECT", "Esc / q", "Exit copy mode", "direct"),
        ];
        let unbound = filtered_help_groups(rows.clone(), HelpTab::Unbound, "");
        let global = filtered_help_groups(rows.clone(), HelpTab::Global, "");
        let modes = filtered_help_groups(rows, HelpTab::Modes, "");
        assert!(
            unbound
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| row.keys.is_empty())
        );
        assert!(
            global
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| !row.keys.is_empty() && row.kind == super::HelpKind::Global)
        );
        assert!(
            modes
                .iter()
                .flat_map(|(_, rows)| rows)
                .all(|row| row.kind == super::HelpKind::Direct)
        );
    }

    #[test]
    fn scrollback_filter_finds_an_unbound_row() {
        let groups = filtered_help_groups(
            [HelpRow::unbound("App", "Edit scrollback")],
            HelpTab::Unbound,
            "SCROLLBACK",
        );
        assert_eq!(groups[0].1[0].label, "Edit scrollback");
    }

    #[test]
    fn search_matches_keys_groups_and_mode_names() {
        let rows = vec![
            HelpRow::global("Panes", "Shift+Enter", "New floating pane"),
            HelpRow::direct(
                "Sidebar focused · DIRECT",
                "Ctrl+Shift+←",
                "Reorder the active tab",
                "direct sidebar focused",
            ),
            HelpRow::unbound("App", "Open config file"),
        ];
        let floating = filtered_help_groups(rows.clone(), HelpTab::Global, "shift enter");
        assert_eq!(floating[0].1[0].label, "New floating pane");
        let sidebar = filtered_help_groups(rows.clone(), HelpTab::Modes, "ctrl shift left");
        assert_eq!(sidebar[0].1[0].label, "Reorder the active tab");
        let unbound = filtered_help_groups(rows, HelpTab::Unbound, "unbound");
        assert_eq!(unbound[0].1[0].label, "Open config file");
    }

    #[test]
    fn help_tab_defaults_to_global() {
        assert_eq!(HelpTab::default(), HelpTab::Global);
    }

    #[test]
    fn scheme_rows_lead_global_and_all_without_a_group() {
        let rows = scheme_rows(&crate::config::InputConfig::default());
        assert_eq!(rows[0].category, "");
        assert_eq!(rows[0].keys, "Ctrl+a");
        assert_eq!(rows[0].label, "Prefix · then key");
        assert_eq!(rows[1].label, "Mod · hold + key");
        assert_eq!(rows[1].keys, "Alt");
        let global = filtered_help_groups(rows.clone(), HelpTab::Global, "");
        assert_eq!(global[0].0, "");
        assert_eq!(global[0].1.len(), 2);
        assert!(filtered_help_groups(rows.clone(), HelpTab::Modes, "").is_empty());
        assert!(filtered_help_groups(rows.clone(), HelpTab::Unbound, "").is_empty());
        assert_eq!(filtered_help_groups(rows, HelpTab::All, "")[0].0, "");
    }

    #[test]
    fn scheme_rows_omit_mod_when_modifier_shortcuts_are_off() {
        let input = crate::config::InputConfig {
            modifier_shortcuts: false,
            ..crate::config::InputConfig::default()
        };
        let rows = scheme_rows(&input);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].label, "Prefix · then key");
    }
}
