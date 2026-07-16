mod agents;
mod panes;
mod sessions;
mod user_tabs;

use tui_lipan::prelude::*;

use crate::config::SidebarTab;
use crate::{HyprmuxApp, Msg};

pub(super) fn sidebar(ctx: &Context<HyprmuxApp>, width: u16) -> Element {
    let tabs = &ctx.state.config.sidebar.tabs;
    let active = ctx
        .state
        .sidebar
        .active_tab
        .as_ref()
        .and_then(|id| tabs.iter().position(|tab| tab.id() == *id))
        .unwrap_or(0);
    let tab_bar = tabs.iter().fold(Tabs::new(), |bar, tab| {
        bar.tab(Tab::new(tab.label().to_string()))
    });
    let tab_ids: Vec<_> = tabs.iter().map(SidebarTab::id).collect();
    let tab_bar =
        tab_bar
            .active(active)
            .on_change(ctx.link().callback(move |event: TabsEvent| {
                Msg::SidebarTabSelected(tab_ids[event.index].clone())
            }));

    let body = tabs.get(active).map_or_else(
        || placeholder(ctx, "No sidebar tabs configured"),
        |tab| match tab {
            SidebarTab::Panes => panes::panes_tab(ctx),
            SidebarTab::Agents => agents::agents_tab(ctx),
            SidebarTab::Sessions => sessions::sessions_tab(ctx),
            SidebarTab::Launcher { name, entries, .. } => {
                user_tabs::launcher_tab(ctx, name, entries)
            }
            SidebarTab::Command { name, on_click, .. } => {
                user_tabs::command_tab(ctx, name, on_click.is_some())
            }
        },
    );

    Frame::new()
        .border(true)
        .border_style(BorderStyle::Plain)
        .padding(0)
        .style(
            ctx.state
                .theme
                .primary
                .patch(Style::new().bg(ctx.state.theme.surface.element)),
        )
        .width(Length::Px(width))
        .height(Length::Flex(1))
        .child(
            VStack::new()
                .gap(0)
                .child(tab_bar)
                .child(ScrollView::new().scrollbar(true).child(body)),
        )
        .into()
}

fn placeholder(ctx: &Context<HyprmuxApp>, text: &str) -> Element {
    VStack::new()
        .padding((1, 1))
        .child(Text::new(text.to_string()).style(super::fg_only(&ctx.state.theme.muted)))
        .into()
}
