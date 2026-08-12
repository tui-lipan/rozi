pub(crate) fn palette_overlay(ctx: &Context<AppRoot>) -> Element {
    // Commands (labels, categories, live keybinding hints, and the handler to run) come
    // straight from the registry `commands.rs` builds. Only palette-eligible ids appear here
    // (see `commands::is_palette_eligible`); the help overlay remains the full reference,
    // including frequent single-key actions this intentionally omits. Group by category
    // (first-seen order) so each category header appears once even when entries of the same
    // category aren't registered contiguously.
    let mut groups: Vec<(String, Vec<SearchEntry<Callback<()>>>)> = Vec::new();
    let mut item_index = 0;
    let mut toggle_sidebar_index = None;
    for entry in ctx.command_registry().entries() {
        if !crate::commands::is_palette_eligible(entry.id.as_str())
            || Action::from_id(entry.id.as_str())
                .is_some_and(|action| !crate::commands::command_available(action, &ctx.state))
        {
            continue;
        }
        if entry.id.as_str() == "toggle-sidebar" {
            toggle_sidebar_index = Some(item_index);
        }
        let category = entry.category.as_deref().unwrap_or("Other").to_string();
        let mut item = SearchEntry::Item(
            SearchItem::new(entry.label.to_string(), entry.handler.clone())
                .aliases(command_palette_aliases(entry.id.as_str()))
                .priority(i32::from(
                    ctx.state.command_palette_sidebar_query
                        && entry.id.as_str() == "toggle-sidebar",
                )),
        );
        item_index += 1;
        let hint = entry.keybinding_hint.as_deref().unwrap_or("");
        if !hint.is_empty() {
            item = item.description(ItemDescription::new().right(hint.to_string()));
        }
        match groups.iter_mut().find(|(name, _)| *name == category) {
            Some((_, items)) => items.push(item),
            None => groups.push((category, vec![item])),
        }
    }

    let entries = command_entries_with_groups(groups);
    let palette = action_search_palette(ctx, entries, "Search commands…")
        .initial_selected_item_index(
            ctx.state
                .command_palette_sidebar_query
                .then_some(toggle_sidebar_index)
                .flatten(),
        )
        .on_query_change(ctx.link().callback(|query: Arc<str>| {
            Msg::CommandPaletteQueryChanged(query.to_string())
        }));

    action_palette(
        ctx,
        "Commands",
        palette_key(),
        Msg::ClosePalette,
        palette,
        60,
    )
}

/// Flatten command groups with one non-selectable blank row between adjacent sections. SearchPalette
/// hides these structural entries while fuzzy results are score-ordered.
fn command_entries_with_groups<T>(
    groups: impl IntoIterator<Item = (impl Into<Arc<str>>, Vec<SearchEntry<T>>)>,
) -> Vec<SearchEntry<T>> {
    let mut entries = Vec::new();
    for (index, (category, items)) in groups.into_iter().enumerate() {
        if index > 0 {
            entries.push(SearchEntry::spacer());
        }
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }
    entries
}

fn command_palette_aliases(id: &str) -> Vec<Arc<str>> {
    match id {
        "settings" => alias_list(&[
            "settings",
            "preferences",
            "configuration",
            "alerts",
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
            "notifications",
            "sound",
            "desktop",
            "blocked",
            "urgent",
            "focus on hover",
        ]),
        "toggle-do-not-disturb" => alias_list(&["dnd", "mute", "quiet"]),
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
        // The live label leads with Enable/Disable; keep the stable noun searchable as an exact hit.
        "toggle-sidebar" => alias_list(&["sidebar"]),
        _ => Vec::new(),
    }
}

fn settings_palette_aliases(group: &str, action: SettingsAction) -> Vec<Arc<str>> {
    use SettingsAction::*;

    let mut aliases = match action {
        Theme => alias_list(&["themes", "color scheme", "colour scheme"]),
        EditPadding => {
            alias_list(&["pane padding", "terminal insets", "pane margins"])
        }
        ToggleTitles => alias_list(&[
            "title bar",
            "show titles",
            "toggle titlebar",
        ]),
        CycleTitleStyle => alias_list(&[
            "titlebar cap style",
            "titlebar caps",
            "titlebar style",
            "titlebar pill",
            "titlebar arrow",
        ]),
        CycleTitlebar => alias_list(&[
            "titlebar layout",
            "titlebar mode",
            "bar titlebar",
            "border titlebar",
            "integrated titlebar",
        ]),
        ToggleWorkbar => alias_list(&["show workbar", "toggle workbar"]),
        ToggleWorkbarGap => {
            alias_list(&["workbar gap", "workbar spacing", "workbar separator"])
        }
        ToggleWorkbarPosition => alias_list(&[
            "workbar position",
            "workbar placement",
            "workbar top",
            "workbar bottom",
        ]),
        CycleWorkbarStyle => {
            alias_list(&["workbar style", "workbar caps", "workbar pill"])
        }
        CycleWorkbarBadgeStyle => alias_list(&[
            "workbar badge style",
            "workbar badges",
            "workbar chips",
        ]),
        ToggleWorkbarPowerline => {
            alias_list(&["workbar powerline", "workbar badge chain"])
        }
        CycleWorkbarTabStyle => alias_list(&[
            "workbar tab style",
            "workspace tab style",
            "workbar tabs",
        ]),
        ToggleAnimations => {
            alias_list(&["animation effects", "motion effects", "transitions"])
        }
        ToggleHighlightFocusedBackground => alias_list(&[
            "focused pane background",
            "active pane background",
        ]),
        ToggleHighlightFocusedBorder => alias_list(&[
            "focused pane border",
            "active pane border",
        ]),
        ToggleHighlightFocusedTitlebar => alias_list(&[
            "focused pane titlebar",
            "active pane titlebar",
        ]),
        CycleBorderMode => alias_list(&[
            "border mode",
            "border merge",
            "merge borders",
            "borderless panes",
            "pane dividers",
        ]),
        ToggleBackgroundFollowsTerminal => alias_list(&[
            "background follows terminal",
            "terminal background",
            "match terminal",
        ]),
        CycleBorderStyle => alias_list(&[
            "border style",
            "rounded borders",
            "square borders",
            "border glyphs",
        ]),
        ToggleFocusOnHover => alias_list(&["mouse focus", "hover focus"]),
        ToggleBellUrgency => alias_list(&["terminal bell", "urgent bell"]),
        CycleAlertBorder => alias_list(&[
            "blocked pane border",
            "agent border",
            "attention border",
            "alert pulse",
        ]),
        CycleWorkbarAlert => alias_list(&[
            "workspace tab alert",
            "workspace marker",
            "tab pulse",
        ]),
        CycleWorkbarAlertPaint => {
            alias_list(&["workspace tab alert paint", "marker fill"])
        }
        CycleStartupMode => alias_list(&[
            "session startup",
            "launch",
            "bare launch",
            "picker",
            "ephemeral",
            "last session",
        ]),
        DefaultProfile => alias_list(&["profile default", "startup profile", "default recipe"]),
        ToggleSessionAutosave => {
            alias_list(&["session autosave", "restore layout", "save layout on quit"])
        }
        ToggleSessionResurrect => {
            alias_list(&["session resurrect", "restore sessions", "server restart"])
        }
        _ => Vec::new(),
    };
    aliases.push(Arc::from(group));
    aliases
}

fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}
