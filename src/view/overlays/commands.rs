use super::*;

/// Whether `registry_id` names a `[[commands]]` entry its extension has hidden from the palette.
fn hidden_named_command(ctx: &Context<AppRoot>, registry_id: &str) -> bool {
    registry_id.strip_prefix("command.").is_some_and(|id| {
        ctx.state.config.commands.iter().any(|command| {
            command.id == id && !crate::ops::command_visibility::palette_lists(&ctx.state, command)
        })
    })
}

/// Commands while the extension command it handed off to is still running: the list no longer
/// responds to anything, so it says what it is waiting on and that leaving does not stop it.
fn handoff_running(ctx: &Context<AppRoot>, label: &str) -> Element {
    let theme = &ctx.state.theme;
    let body = VStack::new()
        .height(Length::Auto)
        .padding((1, 2, 1, 2))
        .child(
            Spinner::new()
                .spinner_style(SpinnerStyle::Dots)
                .label(format!("Running “{label}”…"))
                .style(Style::new().fg(theme.status.info))
                .label_style(fg_only(&theme.primary)),
        )
        .child(
            Text::new("Esc closes this; the command keeps running.")
                .style(fg_only(&theme.muted))
                .height(Length::Px(1)),
        );
    action_palette(ctx, "Commands", palette_key(), Msg::ClosePalette, body, 60)
}

pub(crate) fn palette_overlay(ctx: &Context<AppRoot>) -> Element {
    if let Some(handoff) = ctx
        .state
        .command_palette_handoff
        .as_ref()
        .filter(|handoff| handoff.running)
    {
        return handoff_running(ctx, &handoff.label);
    }
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
                .is_some_and(|action| !crate::commands::palette_visible(action, &ctx.state))
            || hidden_named_command(ctx, entry.id.as_str())
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
        "release-notes" => alias_list(&["release notes", "changelog", "changes", "news"]),
        "new-temporary-session" => alias_list(&["ephemeral"]),
        "screenshot-pane" | "screenshot-ui" => alias_list(&["capture", "png", "image", "snapshot"]),
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

pub(super) fn alias_list(values: &[&str]) -> Vec<Arc<str>> {
    values.iter().copied().map(Arc::from).collect()
}
