use super::*;

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
        entries.push(SearchEntry::Item(
            SearchItem::new(
                tab.map(|tab| tab.label().to_string())
                    .unwrap_or_else(|| id.clone()),
                id.clone(),
            )
            .alias(id.clone())
            .alias(status)
            .description(picker_description(status)),
        ));
        statuses.insert(id, (enabled, available));
    }
    let statuses = Arc::new(statuses);
    let gutter_statuses = Arc::clone(&statuses);
    let accent = fg_only(&ctx.state.theme.accent);
    let muted = fg_only(&ctx.state.theme.muted);
    let palette =
        shared_search_palette::<String>(ctx, Length::Auto, true)
            .entries(entries)
            .placeholder("Search tabs…")
            .item_gutter(Arc::new(move |item, _| {
                let &(enabled, available) = gutter_statuses.get(&item.value)?;
                Some(ListItemGutter::from_spans([Span::new(if !available {
                    "– "
                } else if enabled {
                    "● "
                } else {
                    "○ "
                })
                .style(if available && enabled { accent } else { muted })]))
            }))
            .render_item(Arc::new(move |item, _| {
                let &(enabled, available) = statuses.get(&item.value)?;
                if enabled && available {
                    return None;
                }
                Some(picker_row(
                    [Span::new(item.label.clone()).style(muted)],
                    if available { "Disabled" } else { "Unavailable" },
                    muted,
                ))
            }))
            .on_activate(ctx.link().callback(|event: SearchEvent<String>| {
                Msg::SidebarManagerActivate(event.item.value)
            }));
    action_palette(
        ctx,
        "Sidebar tabs",
        "sidebar-manager",
        Msg::SidebarManagerBack,
        palette,
        65,
    )
}
