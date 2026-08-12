pub(crate) fn help_overlay(ctx: &Context<AppRoot>) -> Element {
    let theme = &ctx.state.theme;
    let prefix = ctx.state.config.input.prefix.to_string();
    let intro = if ctx.state.config.input.modifier_shortcuts {
        let modifier = ctx.state.config.input.modifier.label();
        format!(
            "Prefix keys with {prefix}, or hold {modifier} with any listed key. Scroll for more · Esc closes."
        )
    } else {
        format!("Prefix keys with {prefix}. Scroll for more · Esc closes.")
    };

    // Group commands by category (first-seen order), so a category with non-contiguous
    // entries still gets a single header - matching the command palette. Commands (labels,
    // categories, live keybinding hints) come straight from the registry `commands.rs` builds,
    // so this always reflects the actual active bindings, including `[keys]` overrides and
    // user-defined commands (registered under category "Custom").
    let mut groups: Vec<(String, Vec<(String, String)>)> = Vec::new();
    for entry in ctx.command_registry().entries() {
        if entry.id.as_str() == crate::commands::FORWARD_PREFIX_COMMAND_ID {
            continue;
        }
        // `detach` and `quit` leave the client in exactly the same way. Keep the detach command
        // registered for its stable id and bindings, but show both aliases on the canonical Quit
        // client row instead of making the help reader compare duplicate actions.
        if entry.id.as_str() == "detach" {
            continue;
        }
        // tui-lipan registers its own `app.*` commands even when their bindings are disabled.
        // Hyprmux owns these behaviors, so only show its corresponding commands here.
        if entry.id.as_str().starts_with("app.") {
            continue;
        }
        // Workspace digit switches are described generically below rather than as 27 rows.
        if entry.id.as_str().starts_with("workspace.") {
            continue;
        }
        let category = entry.category.as_deref().unwrap_or("Other").to_string();
        let row = (
            entry.keybinding_hint.as_deref().unwrap_or("").to_string(),
            entry.label.to_string(),
        );
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, rows)) => rows.push(row),
            None => groups.push((category, vec![row])),
        }
    }
    // Workspace digits and mouse gestures aren't individual registry entries; append them.
    groups.push((
        "Workspaces".to_string(),
        vec![
            ("1-9".to_string(), "Switch to workspace".to_string()),
            (
                "Shift+1-9".to_string(),
                "Move pane to workspace (follow)".to_string(),
            ),
            (
                "Ctrl+Shift+1-9".to_string(),
                "Move workspace to workspace (follow)".to_string(),
            ),
        ],
    ));
    groups.push((
        "Mouse".to_string(),
        vec![
            (
                "mod/prefix-drag".to_string(),
                "Move pane (left-drag)".to_string(),
            ),
            (
                "mod/prefix-right-drag".to_string(),
                "Resize pane from corner".to_string(),
            ),
            ("drag gap".to_string(), "Resize a tiled split".to_string()),
        ],
    ));
    // User `[keys]` commands are already covered by the registry loop above (category
    // "Custom"). Copy mode's internal keys aren't discrete commands, so they aren't registered.
    groups.push((
        "Copy mode".to_string(),
        vec![
            ("hjkl / arrows".to_string(), "Move cursor".to_string()),
            (
                "w / b / e".to_string(),
                "Word forward / back / end".to_string(),
            ),
            (
                "W / B / E".to_string(),
                "WORD forward / back / end".to_string(),
            ),
            (
                "0 / ^ / $".to_string(),
                "Line start / first non-blank / end".to_string(),
            ),
            (
                "g / G".to_string(),
                "Top / bottom of scrollback".to_string(),
            ),
            (
                "Ctrl+u / Ctrl+d".to_string(),
                "Half page up / down".to_string(),
            ),
            ("v / Space".to_string(), "Start selection".to_string()),
            ("y / Enter".to_string(), "Copy selection & exit".to_string()),
            ("Esc / q".to_string(), "Exit copy mode".to_string()),
        ],
    ));
    groups.sort_by_key(|(category, _)| help_category_priority(category));

    let mut list = VStack::new();
    for (index, (category, rows)) in groups.iter().enumerate() {
        list = list.child(help_section(category, theme, index > 0));
        for (keys, label) in rows {
            list = list.child(help_row(keys, label, theme));
        }
    }

    let body = VStack::new()
        .child(
            Text::new(intro)
                .style(theme.muted)
                .overflow(Overflow::Wrap)
                .height(Length::Auto),
        )
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

    styled_modal(ctx, "Keybindings", 60)
        .height(Length::Percent(70))
        .padding((1, 1, 1, 2))
        .on_close(ctx.link().callback(|_| Msg::CloseHelp))
        .child(body)
        .into()
}

fn help_category_priority(category: &str) -> usize {
    match category {
        "App" => 0,
        "Session" => 1,
        "Collaboration" => 2,
        "Panes" => 3,
        "Focus" => 4,
        "Layout" => 5,
        "Workspaces" => 6,
        "Copy mode" => 7,
        "Profile" => 8,
        "Appearance" => 9,
        "Mouse" => 10,
        "Custom" => 11,
        _ => 12,
    }
}

fn help_section(title: &str, theme: &Theme, spaced: bool) -> Element {
    // A horizontal rule with the section title on it - the title in the accent color, the
    // line muted - so groups read as clear dividers without competing with the key text.
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
        ("not set".to_string(), fg_only(&theme.muted))
    } else {
        (
            keys.to_string(),
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
    use super::{appearance_palette_aliases, command_palette_aliases, help_category_priority};
    use crate::state::AppearanceAction;

    #[test]
    fn every_appearance_action_has_search_aliases() {
        let actions = [
            AppearanceAction::Theme,
            AppearanceAction::EditPadding,
            AppearanceAction::ToggleTitles,
            AppearanceAction::CycleTitlebar,
            AppearanceAction::CycleTitleStyle,
            AppearanceAction::ToggleWorkbar,
            AppearanceAction::ToggleWorkbarGap,
            AppearanceAction::ToggleWorkbarPosition,
            AppearanceAction::CycleWorkbarStyle,
            AppearanceAction::CycleWorkbarBadgeStyle,
            AppearanceAction::ToggleWorkbarPowerline,
            AppearanceAction::CycleWorkbarTabStyle,
            AppearanceAction::ToggleAnimations,
            AppearanceAction::ToggleHighlightFocusedBackground,
            AppearanceAction::ToggleHighlightFocusedBorder,
            AppearanceAction::ToggleHighlightFocusedTitlebar,
            AppearanceAction::CycleBorderMode,
            AppearanceAction::CycleBorderStyle,
            AppearanceAction::ToggleBackgroundFollowsTerminal,
        ];
        for action in actions {
            assert!(
                !appearance_palette_aliases(action).is_empty(),
                "missing aliases for {action:?}"
            );
        }
    }

    #[test]
    fn change_appearance_command_aliases_cover_recent_controls() {
        let aliases = command_palette_aliases("change-appearance");
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
                "missing change-appearance alias: {term}"
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
    }

    #[test]
    fn help_categories_put_appearance_after_profiles() {
        let categories = [
            "Appearance",
            "Layout",
            "Session",
            "Collaboration",
            "App",
            "Panes",
            "Profile",
            "Custom",
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
                "Layout",
                "Profile",
                "Appearance",
                "Custom",
            ]
        );
    }
}
