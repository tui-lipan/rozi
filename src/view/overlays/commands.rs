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

    let mut entries: Vec<SearchEntry<Callback<()>>> = Vec::new();
    for (category, items) in groups {
        entries.push(SearchEntry::header(category));
        entries.extend(items);
    }
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
        // The live label is "Enable/Disable sidebar", so a bare "sidebar" query only matches its
        // label as a mid-string substring and loses to "Next/Previous sidebar tab". An exact
        // "sidebar" alias competes via `max()` and floats the toggle to the top.
        "toggle-sidebar" => alias_list(&["sidebar", "panel", "toggle sidebar"]),
        _ => Vec::new(),
    }
}

fn appearance_palette_aliases(action: AppearanceAction) -> Vec<Arc<str>> {
    match action {
        AppearanceAction::Theme => alias_list(&[
            "theme", "themes", "color", "colors", "colour", "colours", "scheme",
        ]),
        AppearanceAction::EditPadding => alias_list(&[
            "padding", "margin", "margins", "inset", "insets", "terminal", "pane",
        ]),
        AppearanceAction::ToggleTitles => alias_list(&[
            "titlebar",
            "titlebars",
            "title",
            "titles",
            "title bar",
            "show titles",
        ]),
        AppearanceAction::CycleTitleStyle => alias_list(&[
            "titlebar cap style",
            "title cap style",
            "titlebar caps",
            "title caps",
            "pill",
            "round",
            "arrow",
            "half",
            "padded",
            "cap style",
        ]),
        AppearanceAction::CycleTitlebar => alias_list(&[
            "titlebar layout",
            "titlebar mode",
            "title layout",
            "bar",
            "border",
            "integrated",
        ]),
        AppearanceAction::ToggleWorkbar => {
            alias_list(&["workbar", "top bar", "status bar", "bar", "show workbar"])
        }
        AppearanceAction::ToggleWorkbarGap => {
            alias_list(&["workbar gap", "gap", "spacing", "gutter", "separator"])
        }
        AppearanceAction::ToggleWorkbarPosition => alias_list(&[
            "workbar position",
            "position",
            "placement",
            "top",
            "bottom",
            "relocate",
        ]),
        AppearanceAction::CycleWorkbarStyle => {
            alias_list(&["workbar style", "workbar caps", "bar style", "bar caps"])
        }
        AppearanceAction::CycleWorkbarBadgeStyle => alias_list(&[
            "badge",
            "badges",
            "badge style",
            "workbar badge",
            "chip",
            "chips",
            "mode chip",
            "powerline badge",
        ]),
        AppearanceAction::ToggleWorkbarPowerline => {
            alias_list(&["powerline", "chain", "chained", "interlock", "badge chain"])
        }
        AppearanceAction::CycleWorkbarTabStyle => {
            alias_list(&["tab", "tabs", "workspace tabs", "tab style", "workbar tabs"])
        }
        AppearanceAction::ToggleAnimations => alias_list(&[
            "animation",
            "animations",
            "motion",
            "transitions",
            "animate",
        ]),
        AppearanceAction::ToggleHighlightFocusedBackground => alias_list(&[
            "focused background",
            "focus background",
            "highlight background",
            "active background",
            "focused pane",
        ]),
        AppearanceAction::ToggleHighlightFocusedBorder => alias_list(&[
            "focused border",
            "focus border",
            "highlight border",
            "active border",
        ]),
        AppearanceAction::ToggleHighlightFocusedTitlebar => alias_list(&[
            "focused titlebar",
            "focus titlebar",
            "highlight titlebar",
            "active titlebar",
        ]),
        AppearanceAction::ToggleBorderMerge => alias_list(&[
            "border merge",
            "merge borders",
            "merging",
            "seam",
            "border seam",
        ]),
        AppearanceAction::ToggleBackgroundFollowsTerminal => alias_list(&[
            "background follows terminal",
            "terminal background",
            "match terminal",
            "transparent background",
            "backdrop",
        ]),
        AppearanceAction::CycleBorderStyle => {
            alias_list(&["border style", "rounded", "square", "border caps"])
        }
    }
}

fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}
