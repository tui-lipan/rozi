use super::*;

pub(crate) fn sidebar_manager_overlay(ctx: &Context<AppRoot>) -> Element {
    let layout = &ctx.state.config.sidebar.layout;
    let mut entries = Vec::new();
    let (title, key) = if ctx.state.sidebar_manager_presets {
        entries.push(SearchEntry::item("Default layout", "default".to_string()));
        for preset in &ctx.state.config.sidebar.presets {
            entries.push(SearchEntry::item(preset.label.clone(), preset.name.clone()));
        }
        (
            "Apply sidebar layout preset".to_string(),
            "sidebar-manager-presets",
        )
    } else if let Some(id) = ctx.state.sidebar_manager_tab.as_deref() {
        let hidden = layout.hidden.iter().any(|tab| tab == id);
        entries.push(SearchEntry::item(
            if hidden { "Show tab" } else { "Hide tab" },
            "visibility".to_string(),
        ));
        let available = ctx
            .state
            .config
            .sidebar
            .tabs
            .iter()
            .any(|tab| tab.id().as_str() == id);
        if available {
            entries.push(SearchEntry::item("Locate tab", "locate".to_string()));
        }
        for side in [
            crate::config::SidebarPosition::Left,
            crate::config::SidebarPosition::Right,
        ] {
            for panel in 0..layout.dock(side).panel_count {
                entries.push(SearchEntry::item(
                    format!("{} · panel {}", side.label(), panel + 1),
                    format!("{}:{panel}", side.id()),
                ));
            }
        }
        (id.to_string(), "sidebar-manager-detail")
    } else {
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
        for id in ids {
            let tab = ctx
                .state
                .config
                .sidebar
                .tabs
                .iter()
                .find(|tab| tab.id().as_str() == id);
            let status = if tab.is_none() {
                "Unavailable"
            } else if layout.hidden.contains(&id) {
                "Hidden"
            } else {
                "Available"
            };
            let location = layout
                .location(&id)
                .map(|(side, panel)| format!("{} · {}", side.label(), panel + 1))
                .unwrap_or_default();
            entries.push(
                SearchEntry::Item(
                    SearchItem::new(
                        tab.map(|tab| tab.label().to_string())
                            .unwrap_or_else(|| id.clone()),
                        id.clone(),
                    )
                    .alias(id)
                    .alias(location.clone()),
                )
                .description(picker_description(format!("{status} · {location}"))),
            );
        }
        ("Manage Sidebar Tabs".to_string(), "sidebar-manager")
    };
    let actions = [OverlayAction::new(
        "ctrl-p",
        "layout presets",
        Msg::SidebarManagerPresets,
        !ctx.state.sidebar_manager_presets,
    )];
    let palette =
        shared_search_palette::<String>(ctx, Length::Auto, true)
            .entries(entries)
            .input_key_interceptor(overlay_interceptor(ctx, &actions))
            .placeholder("Search tabs or locations…")
            .on_activate(ctx.link().callback(|event: SearchEvent<String>| {
                Msg::SidebarManagerActivate(event.item.value)
            }));
    let body = VStack::new()
        .height(Length::Auto)
        .child(palette)
        .child(overlay_hints(ctx, &actions));
    action_palette(ctx, &title, key, Msg::SidebarManagerBack, body, 65)
}
