use super::*;

use crate::view::vocabulary;

pub(crate) fn sidebar_manager_overlay(ctx: &Context<AppRoot>) -> Element {
    let layout = &ctx.state.config.sidebar.layout;
    let mut ids: Vec<_> = ctx
        .state
        .config
        .sidebar
        .tabs
        .iter()
        .map(|tab| tab.id().as_str().to_string())
        .collect();
    for side in [
        crate::config::SidebarPosition::Left,
        crate::config::SidebarPosition::Right,
    ] {
        for panel in &layout.dock(side).panels {
            for id in &panel.tabs {
                if !ids.contains(id) {
                    ids.push(id.clone());
                }
            }
        }
    }
    let mut entries = Vec::new();
    let mut statuses = std::collections::HashMap::new();
    for id in ids {
        let tab = ctx
            .state
            .config
            .sidebar
            .tabs
            .iter()
            .find(|tab| tab.id().as_str() == id);
        let enabled = !layout.hidden.contains(&id);
        let available = tab.is_some();
        let status = if !available {
            "Unavailable"
        } else if enabled {
            "Enabled"
        } else {
            "Disabled"
        };
        // How many settings Ctrl+E opens, so a tab that has some says so before it is highlighted.
        let settings = ctx
            .state
            .config
            .sidebar
            .tab_settings
            .get(&id)
            .map_or(0, |group| group.specs.len());
        let description = vocabulary::fields([
            status.to_string(),
            if settings > 0 {
                vocabulary::count(settings, "setting")
            } else {
                String::new()
            },
        ]);
        let mut item = SearchItem::new(
            tab.map(|tab| tab.label().to_string())
                .unwrap_or_else(|| id.clone()),
            id.clone(),
        )
        .alias(id.clone())
        .alias(status)
        .description(picker_description(&description));
        if settings > 0 {
            item = item.alias("settings");
        }
        entries.push(SearchEntry::Item(item));
        statuses.insert(id, (enabled, available, description));
    }
    let manager = ctx.state.sidebar_manager.clone().unwrap_or_default();
    let selected = manager.selected.as_deref().and_then(|selected| {
        entries.iter().position(|entry| match entry {
            SearchEntry::Item(item) => item.value == selected,
            _ => false,
        })
    });
    let has_settings = manager
        .selected
        .as_deref()
        .is_some_and(|id| crate::ops::tab_settings::tab_has_settings(&ctx.state, id));
    let actions = vec![
        OverlayAction::new(
            "enter",
            "toggle",
            Msg::SidebarManagerActivate(manager.selected.clone().unwrap_or_default()),
            true,
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-e",
            "settings",
            Msg::SidebarManagerSettings,
            has_settings,
        ),
    ];
    let statuses = Arc::new(statuses);
    let gutter_statuses = Arc::clone(&statuses);
    let accent = fg_only(&ctx.state.theme.accent);
    let muted = fg_only(&ctx.state.theme.muted);
    OverlayPalette::new(
        "Sidebar tabs",
        "sidebar-manager",
        Msg::SidebarManagerBack,
        65,
    )
    .entries(entries)
    .actions(actions)
    .selected(selected)
    .initial_query(manager.query)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::SidebarManagerQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::SidebarManagerSelect(event.item.value)),
    )
    .placeholder("Search tabs…")
    .item_gutter(Arc::new(move |item, _| {
        let &(enabled, available, _) = gutter_statuses.get(&item.value)?;
        Some(vocabulary::picker_marker_gutter(
            if !available {
                "–"
            } else if enabled {
                "●"
            } else {
                "○"
            },
            if available && enabled { accent } else { muted },
        ))
    }))
    .render_item(Arc::new(move |item, _| {
        let (enabled, available, description) = statuses.get(&item.value)?;
        if *enabled && *available {
            return None;
        }
        Some(picker_row(
            [Span::new(item.label.clone()).style(muted)],
            description.as_str(),
            muted,
        ))
    }))
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<String>| Msg::SidebarManagerActivate(event.item.value)),
    )
    .render(ctx)
}
