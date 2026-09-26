use super::*;

use crate::state::{PaneId, ScrollbackMatch, SearchScope};
use tui_lipan::style::RowStylePolicy;

pub(crate) fn search_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(search) = ctx.state.search.as_ref() else {
        return Text::new("").into();
    };
    let body = VStack::new()
        .height(Length::Auto)
        .child(scrollback_search_palette(ctx, search))
        .child(scrollback_search_hints(ctx, search));
    let title = format!("Search scrollback · {}", search.scope.label());

    let panel: Element = Frame::new()
        .header_left(title)
        .header_style(rozi_chrome(&ctx.state.theme).bold())
        .border_style(overlay_border_style(ctx))
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(body)
        .into();
    Modal::new()
        .width(Length::Px(90))
        .height(Length::Auto)
        .max_height(Length::Percent(65))
        .reserve_height(Length::Percent(65))
        .border(false)
        .padding(0)
        .frame_style(Style::new().bg(ctx.state.theme.surface.element))
        .on_close(ctx.link().callback(|_| Msg::CloseSearch))
        .child(panel)
        .key(search_input_key())
}

fn scrollback_search_hints(ctx: &Context<AppRoot>, search: &ScrollbackSearchState) -> Element {
    let mut hints = hint_row();
    if !search.from_copy_mode {
        hints = hints.child(hint_button(
            ctx,
            "change scope",
            "tab",
            Msg::SearchCycleScope,
        ));
    }
    hints.into()
}

fn scrollback_search_palette(
    ctx: &Context<AppRoot>,
    search: &ScrollbackSearchState,
) -> SearchPalette<usize> {
    let current = search.current;
    let query = search.input.text().trim();

    let empty_text = scrollback_search_empty_text(search, query);
    let matches: Arc<[ScrollbackMatch]> = search.matches.clone().into();
    let item_style = fg_only(&ctx.state.theme.primary);
    let description_style = fg_only(&ctx.state.theme.muted);
    let match_style = crate::view::search_palette_item_match_style(&ctx.state.theme);

    let palette = shared_search_palette::<usize>(ctx, Length::Auto, false);
    let palette = if search.scope == SearchScope::FocusedPane {
        palette.items_arc(Arc::clone(&search.items))
    } else {
        palette
            .entries(scrollback_search_entries(ctx, search))
            .preserve_groups(true)
    };

    palette
        .sync_match_limit(MAX_MATCHES)
        .placeholder("Search scrollback…")
        .initial_query(query.to_string())
        .initial_selected_item_index(Some(current))
        .sync_selection(true)
        .preserve_item_order(true)
        .description_placement(DescriptionPlacement::Right)
        .primary_truncate_description_first(false)
        .empty_text(empty_text)
        .render_item(Arc::new(move |item, _highlight| {
            let matched = matches.get(item.value)?;
            Some(scrollback_search_item(
                item,
                matched,
                item_style,
                description_style,
                match_style,
            ))
        }))
        .input_key_interceptor(scrollback_search_key_interceptor(ctx))
        .on_query_change(
            ctx.link()
                .callback(|query: std::sync::Arc<str>| Msg::SearchQueryChanged(query.to_string())),
        )
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SearchSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SearchActivate(event.item.value)),
        )
}

fn scrollback_search_entries(
    ctx: &Context<AppRoot>,
    search: &ScrollbackSearchState,
) -> Vec<SearchEntry<usize>> {
    let mut entries = Vec::with_capacity(search.items.len() + search.matches.len());
    let mut previous_pane = None;
    let mut pane_number = 0;
    for (item, matched) in search.items.iter().zip(&search.matches) {
        if previous_pane != Some(matched.pane) {
            pane_number += 1;
            entries.push(SearchEntry::header(scrollback_search_group_label(
                ctx,
                matched.pane,
                pane_number,
                search.scope,
            )));
            previous_pane = Some(matched.pane);
        }
        entries.push(SearchEntry::Item(item.clone()));
    }
    entries
}

fn scrollback_search_group_label(
    ctx: &Context<AppRoot>,
    pane_id: PaneId,
    pane_number: usize,
    scope: SearchScope,
) -> String {
    let state = &ctx.state;
    let remote = state.current().remote_target.is_some();
    if let Some((workspace_index, workspace)) = state
        .current()
        .workspaces
        .iter()
        .enumerate()
        .find(|(_, workspace)| workspace.panes.iter().any(|pane| pane.id == pane_id))
        && let Some(pane) = workspace
            .panes
            .iter()
            .find(|pane| pane.id == pane_id && !pane.closing)
    {
        let pane_label = format!("Pane {pane_number}");
        let title = pane.titlebar_title(remote);
        return if scope == SearchScope::All {
            let workspace_label = match workspace.name.as_deref() {
                Some(name) if !name.is_empty() => format!("{}:{name}", workspace_index + 1),
                _ => (workspace_index + 1).to_string(),
            };
            format!("Workspace {workspace_label} · {pane_label} · {title}")
        } else {
            format!("{pane_label} · {title}")
        };
    }

    if let Some(pane) = state
        .scratch
        .panes
        .iter()
        .find(|pane| pane.id == pane_id && !pane.closing)
    {
        let pane_label = format!("Pane {pane_number}");
        let title = pane.titlebar_title(false);
        return if scope == SearchScope::All {
            format!("Scratchpad · {pane_label} · {title}")
        } else {
            format!("{pane_label} · {title}")
        };
    }

    "Pane".to_string()
}

fn scrollback_search_item(
    item: &SearchItem<usize>,
    matched: &ScrollbackMatch,
    item_style: Style,
    description_style: Style,
    match_style: Style,
) -> ListItem {
    let label = item.label.as_ref();
    let leading = matched.text.len() - matched.text.trim_start().len();
    let start_byte = matched.start_byte.saturating_sub(leading).min(label.len());
    let end_byte = matched
        .end_byte
        .saturating_sub(leading)
        .clamp(start_byte, label.len());
    let spans = [
        Span::new(&label[..start_byte]).style(item_style),
        Span::new(&label[start_byte..end_byte])
            .style(item_style.patch(match_style))
            .row_style_policy(RowStylePolicy::PreserveForeground),
        Span::new(&label[end_byte..]).style(item_style),
    ];
    let mut rendered = ListItem::from_spans(spans);
    if let Some(description) = item
        .description
        .as_ref()
        .and_then(|description| description.right.as_ref())
    {
        rendered = rendered
            .description(Arc::clone(description))
            .description_style(description_style)
            .primary_truncate_description_first(false);
    }
    rendered
}

fn scrollback_search_empty_text(search: &ScrollbackSearchState, query: &str) -> String {
    if query.is_empty() {
        "Type to search scrollback".to_string()
    } else if search.scan.is_some() {
        "Scanning…".to_string()
    } else {
        format!("No matches for `{query}`")
    }
}

fn scrollback_search_key_interceptor(ctx: &Context<AppRoot>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSearch)
        } else if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            Some(Msg::SearchCycleScope)
        } else {
            None
        }
    })
}
