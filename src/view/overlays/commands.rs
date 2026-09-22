use super::*;

pub(crate) fn palette_overlay(ctx: &Context<AppRoot>) -> Element {
    // Commands (labels, categories, live keybinding hints, and the handler to run) come
    // straight from the registry `commands/` builds. Only palette-eligible ids appear here
    // (see `commands::is_palette_eligible`); the help overlay remains the full reference,
    // including frequent directional/toggle keys this intentionally omits. Group by category
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
            item = item.description(picker_description(hint));
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
        .on_query_change(
            ctx.link()
                .callback(|query: Arc<str>| Msg::CommandPaletteQueryChanged(query.to_string())),
        );

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
pub(super) fn command_entries_with_groups<T>(
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

pub(super) fn command_palette_aliases(id: &str) -> Vec<Arc<str>> {
    match id {
        "extensions" => alias_list(&["plugins", "addons", "extension manager"]),
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
            "floating border",
            "scratchpad border",
            "fullscreen border",
            "picker border",
            "picker tab",
            "picker selection",
            "picker selection style",
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
            "nerd icons",
            "nerd font",
            "patched font",
            "focused border",
            "focused background",
            "focused titlebar",
            "titlebar focus",
            "which key",
            "which-key",
            "prefix hints",
            "key hints",
            "chord panel",
            "which key delay",
            "background follows terminal",
            "terminal background",
            "background follows canvas",
            "sidebar",
            "sidebar position",
            "sidebar gap",
            "sidebar chrome",
            "sidebar background",
            "sidebar tab",
            "workbar background",
            "notifications",
            "sound",
            "sounds",
            "audio",
            "desktop",
            "blocked",
            "urgent",
            "bell",
            "marker",
            "markers",
            "workspace marker",
            "alert marker",
            "session startup",
            "startup",
            "autosave",
            "resurrect",
            "focus on hover",
        ]),
        "toggle-do-not-disturb" => alias_list(&["dnd", "mute", "quiet"]),
        "update-rozi" => alias_list(&["upgrade", "new version", "release"]),
        "new-temporary-session" => alias_list(&["ephemeral"]),
        "spawn" => alias_list(&["new pane", "split pane", "spawn pane"]),
        "close" => alias_list(&["kill pane", "close focused"]),
        "spawn-float" => alias_list(&["floating pane", "float spawn", "spawn floating"]),
        "collaborators" => alias_list(&[
            "clients",
            "roster",
            "sharing",
            "collaboration",
            "kick",
            "remove client",
            "manage collaborators",
        ]),
        "request-control" => alias_list(&["take control", "layout control", "collaboration"]),
        "toggle-input-lock" => alias_list(&["input lock", "follower input", "collaboration"]),
        "toggle-control-takeover" => alias_list(&["takeover", "control safety", "collaboration"]),
        // The live label leads with Enable/Disable; keep the stable noun searchable as an exact hit.
        "toggle-sidebar" => alias_list(&["sidebar"]),
        "choose-layout" => alias_list(&["choose layout", "switch layout", "layout picker"]),
        // A user command's label is whatever its author called it, so the group name is the only
        // term shared by all of them - the same way "collaboration" reaches that cluster above.
        id if id.starts_with("user.") => alias_list(&["custom"]),
        id if id.starts_with("command.") => {
            let extension = id
                .strip_prefix("command.")
                .and_then(|id| id.split_once('.'))
                .map(|(extension, _)| extension);
            let mut aliases = alias_list(&["custom"]);
            if let Some(extension) = extension {
                aliases.push(Arc::from(extension));
            }
            aliases
        }
        _ => Vec::new(),
    }
}

/// Search terms a row's label and headings do not already carry. The row's section and tab names
/// are appended, and a multi-word query matches term by term across the label and every alias, so
/// "workbar gap" needs no alias of its own. List synonyms, option values, and jargon only.
pub(super) fn settings_palette_aliases(group: &str, action: SettingsAction) -> Vec<Arc<str>> {
    use SettingsAction::*;

    let mut aliases = match action {
        Theme => alias_list(&["color scheme", "colour scheme"]),
        ToggleNerdIcons => alias_list(&["nerd font", "patched font", "glyphs"]),
        CycleWhichKey => alias_list(&["prefix hints", "key hints", "chord panel"]),
        ToggleFocusOnHover => alias_list(&["mouse"]),
        ToggleAnimations => alias_list(&["motion", "transitions", "effects"]),
        ToggleWorkspaceAnimation => alias_list(&["slide"]),
        CycleSessionAnimation => alias_list(&["fade", "portal"]),
        CyclePaneAnimation => alias_list(&["spawn", "scale", "slide", "portal", "scan"]),
        CycleCopyOnSelect => alias_list(&["primary selection", "mouse"]),
        CycleMiddleClickPaste => alias_list(&["primary selection", "mouse"]),
        CycleRightClickClipboard => alias_list(&["copy", "paste", "mouse"]),
        ToggleOsc52 => alias_list(&["osc52", "ssh", "remote"]),
        CyclePickerBorderStyle => alias_list(&["palette", "modal", "overlay"]),
        TogglePickerTabBackground => alias_list(&["palette", "tab bar"]),
        CyclePickerTabStyle => alias_list(&["palette", "caps"]),
        CyclePickerSelectionStyle => alias_list(&["palette", "highlight", "caps", "pill"]),
        ToggleBackgroundFollowsTerminal => alias_list(&["match terminal"]),
        ToggleHighlightFocusedBackground
        | ToggleHighlightFocusedBorder
        | ToggleHighlightFocusedTitlebar => alias_list(&["active", "highlight"]),
        EditPadding => alias_list(&["margins", "insets", "spacing"]),
        CycleBorderMode => alias_list(&["merge", "borderless", "dividers"]),
        CycleBorderStyle => alias_list(&["tiled", "rounded", "square", "dashed"]),
        CycleFloatBorderStyle => alias_list(&["float", "popup"]),
        CycleScratchBorderStyle => alias_list(&["scratch", "dropdown"]),
        CycleFullscreenBorderStyle => alias_list(&["full screen", "maximized", "maximised"]),
        ChooseTitlebar => alias_list(&["title bar", "show titles", "integrated", "inset"]),
        CycleTitleStyle => alias_list(&["caps", "pill"]),
        ChooseWorkbar => alias_list(&["show", "hide", "top", "bottom"]),
        ToggleWorkbarGap | ToggleSidebarGap => alias_list(&["spacing"]),
        ToggleWorkbarBackground => alias_list(&["strip", "fill"]),
        CycleWorkbarStyle => alias_list(&["caps", "pill"]),
        CycleWorkbarBadgeStyle => alias_list(&["badges", "chips"]),
        CycleWorkbarTabStyle => alias_list(&["workspace tabs"]),
        ToggleWorkbarPowerline => alias_list(&["badge chain"]),
        ToggleSidebarPosition => alias_list(&["left", "right", "dock"]),
        ToggleSidebarBackgroundFollowsCanvas => alias_list(&["match canvas", "app background"]),
        ToggleSidebarBackground => alias_list(&["background", "tab bar"]),
        CycleSidebarTabStyle => alias_list(&["caps"]),
        ToggleBellUrgency => alias_list(&["urgent", "terminal"]),
        CycleAlertBorder => alias_list(&["attention", "pulse", "agent"]),
        CycleWorkbarAlert => alias_list(&["attention", "pulse", "marker"]),
        CycleWorkbarAlertPaint => alias_list(&["fill", "color", "colour"]),
        ToggleMarkBell => alias_list(&["marker"]),
        ToggleMarkBlocked => alias_list(&["marker", "waiting"]),
        ToggleMarkFinished => alias_list(&["marker", "done", "completed"]),
        ToggleMarkWorking => alias_list(&["marker", "busy", "running"]),
        ToggleMarkIdle => alias_list(&["marker", "quiet"]),
        ToggleDesktopEnabled => alias_list(&["system", "notify-send", "popups"]),
        ToggleDesktopBlocked => alias_list(&["waiting", "agent prompt"]),
        ToggleDesktopDone | ToggleSoundDone => alias_list(&["done"]),
        ToggleDesktopExit => alias_list(&["quit"]),
        ToggleDesktopExitError | ToggleSoundError => alias_list(&["failure", "non-zero", "crash"]),
        ToggleSoundEnabled => alias_list(&["audio", "mute"]),
        ToggleSoundBell => alias_list(&["beep"]),
        ToggleSoundBlocked => alias_list(&["waiting"]),
        CycleStartupMode => alias_list(&["launch", "picker", "ephemeral", "last session"]),
        ToggleSessionAutosave => alias_list(&["restore"]),
        ToggleSessionResurrect => alias_list(&["restore", "restart"]),
        CycleResurrectForeground => alias_list(&["rerun", "agents", "foreground"]),
    };
    aliases.push(Arc::from(group));
    aliases
}

pub(super) fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}
