pub(crate) fn palette_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    // Commands (labels, categories, live keybinding hints, and the handler to run) come
    // straight from the registry `commands.rs` builds. Only palette-eligible ids appear here
    // (see `commands::is_palette_eligible`); the help overlay remains the full reference,
    // including frequent single-key actions this intentionally omits. Group by category
    // (first-seen order) so each category header appears once even when entries of the same
    // category aren't registered contiguously.
    let mut groups: Vec<(String, Vec<SearchEntry<Callback<()>>>)> = Vec::new();
    for entry in ctx.command_registry().entries() {
        if !crate::commands::is_palette_eligible(entry.id.as_str())
            || Action::from_id(entry.id.as_str())
                .is_some_and(|action| !crate::commands::command_available(action, &ctx.state))
        {
            continue;
        }
        let category = entry.category.as_deref().unwrap_or("Other").to_string();
        let mut item = SearchEntry::Item(
            SearchItem::new(entry.label.to_string(), entry.handler.clone())
                .aliases(command_palette_aliases(entry.id.as_str())),
        );
        let hint = entry.keybinding_hint.as_deref().unwrap_or("");
        if !hint.is_empty() {
            item = item.description(ItemDescription::new().right(hint.to_string()));
        }
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, items)) => items.push(item),
            None => groups.push((category, vec![item])),
        }
    }

    let entries = search_entries_with_groups(groups);
    let palette = action_search_palette(ctx, entries, "Search commands…");

    action_palette(
        ctx,
        "Commands",
        palette_key(),
        Msg::ClosePalette,
        palette,
        60,
    )
}

fn command_palette_aliases(id: &str) -> Vec<Arc<str>> {
    match id {
        "change-appearance" => alias_list(&[
            "theme",
            "themes",
            "appearance",
            "style",
            "chrome",
            "padding",
            "terminal padding",
            "border",
            "borders",
            "border merge",
            "titlebar",
            "titlebars",
            "titlebar style",
            "workbar",
            "top bar",
            "workbar gap",
            "workbar position",
            "workbar style",
            "badge",
            "badges",
            "workbar badge",
            "powerline",
            "tab",
            "tabs",
            "workbar tab",
            "animations",
            "motion",
            "transitions",
            "focused border",
            "focused background",
            "focused titlebar",
            "titlebar focus",
        ]),
        "new-temporary-session" => alias_list(&["ephemeral"]),
        "collaborators" => alias_list(&[
            "clients",
            "roster",
            "sharing",
            "collaboration",
            "kick",
            "remove client",
        ]),
        "request-control" => alias_list(&["take control", "layout control", "collaboration"]),
        "toggle-input-lock" => alias_list(&["input lock", "follower input", "collaboration"]),
        "toggle-control-takeover" => alias_list(&["takeover", "control safety", "collaboration"]),
        // Live label leads with "Sidebar"; keep an exact "sidebar" alias so it stays the first hit
        // even if the display wording shifts, plus common synonyms.
        "toggle-sidebar" => alias_list(&["sidebar", "panel", "toggle sidebar", "enable sidebar", "disable sidebar"]),
        _ => Vec::new(),
    }
}

fn appearance_palette_aliases(action: AppearanceAction) -> Vec<Arc<str>> {
    match action {
        AppearanceAction::Theme => alias_list(&["themes", "color scheme", "colour scheme"]),
        AppearanceAction::EditPadding => {
            alias_list(&["pane padding", "terminal insets", "pane margins"])
        }
        AppearanceAction::ToggleTitles => alias_list(&[
            "title bar",
            "show titles",
            "toggle titlebar",
        ]),
        AppearanceAction::CycleTitleStyle => alias_list(&[
            "titlebar cap style",
            "titlebar caps",
            "titlebar style",
            "titlebar pill",
            "titlebar arrow",
        ]),
        AppearanceAction::CycleTitlebar => alias_list(&[
            "titlebar layout",
            "titlebar mode",
            "bar titlebar",
            "border titlebar",
            "integrated titlebar",
        ]),
        AppearanceAction::ToggleWorkbar => alias_list(&["show workbar", "toggle workbar"]),
        AppearanceAction::ToggleWorkbarGap => {
            alias_list(&["workbar gap", "workbar spacing", "workbar separator"])
        }
        AppearanceAction::ToggleWorkbarPosition => alias_list(&[
            "workbar position",
            "workbar placement",
            "workbar top",
            "workbar bottom",
        ]),
        AppearanceAction::CycleWorkbarStyle => {
            alias_list(&["workbar style", "workbar caps", "workbar pill"])
        }
        AppearanceAction::CycleWorkbarBadgeStyle => alias_list(&[
            "workbar badge style",
            "workbar badges",
            "workbar chips",
        ]),
        AppearanceAction::ToggleWorkbarPowerline => {
            alias_list(&["workbar powerline", "workbar badge chain"])
        }
        AppearanceAction::CycleWorkbarTabStyle => alias_list(&[
            "workbar tab style",
            "workspace tab style",
            "workbar tabs",
        ]),
        AppearanceAction::ToggleAnimations => {
            alias_list(&["animation effects", "motion effects", "transitions"])
        }
        AppearanceAction::ToggleHighlightFocusedBackground => alias_list(&[
            "focused pane background",
            "active pane background",
        ]),
        AppearanceAction::ToggleHighlightFocusedBorder => alias_list(&[
            "focused pane border",
            "active pane border",
        ]),
        AppearanceAction::ToggleHighlightFocusedTitlebar => alias_list(&[
            "focused pane titlebar",
            "active pane titlebar",
        ]),
        AppearanceAction::CycleBorderMode => alias_list(&[
            "border mode",
            "border merge",
            "merge borders",
            "borderless panes",
            "pane dividers",
        ]),
        AppearanceAction::ToggleBackgroundFollowsTerminal => alias_list(&[
            "background follows terminal",
            "terminal background",
            "match terminal",
        ]),
        AppearanceAction::CycleBorderStyle => alias_list(&[
            "border style",
            "rounded borders",
            "square borders",
            "border glyphs",
        ]),
    }
}

fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}
