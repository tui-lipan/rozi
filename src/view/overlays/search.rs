pub(crate) fn search_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(search) = ctx.state.search.as_ref() else {
        return Text::new("").into();
    };
    let body = VStack::new()
        .height(Length::Auto)
        .child(scrollback_search_palette(ctx, search))
        .child(scrollback_search_hints(ctx, search));

    let panel: Element = Frame::new()
        .header_left("Search scrollback")
        .header_right(search.status.clone())
        .header_style(ctx.state.theme.accent.bold())
        .border_style(BorderStyle::Rounded)
        .padding(0)
        .style(Style::new().bg(ctx.state.theme.surface.element))
        .height(Length::Auto)
        .child(action_palette_frame(body))
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

fn scrollback_search_hints(ctx: &Context<HyprmuxApp>, search: &ScrollbackSearchState) -> Element {
    let theme = &ctx.state.theme;
    let mut hints = hint_row()
        .child(hint_pill(theme, "next", "ctrl+n"))
        .child(hint_pill(theme, "previous", "ctrl+p"));
    if !search.from_copy_mode {
        hints = hints.child(hint_pill(theme, search.scope.label(), "tab"));
    }
    hints.into()
}

fn scrollback_search_palette(
    ctx: &Context<HyprmuxApp>,
    search: &ScrollbackSearchState,
) -> SearchPalette<usize> {
    let current = search.current;
    let query = search.input.text().trim();

    let empty_text = scrollback_search_empty_text(search, query);

    shared_search_palette::<usize>(ctx, Length::Auto, true)
        .items_arc(Arc::clone(&search.items))
        .sync_match_limit(MAX_MATCHES)
        .placeholder("Search scrollback...")
        .initial_query(query.to_string())
        .initial_selected_item_index(Some(current))
        .sync_selection(true)
        .description_placement(DescriptionPlacement::Right)
        .empty_text(empty_text)
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

fn scrollback_search_empty_text(search: &ScrollbackSearchState, query: &str) -> String {
    if query.is_empty() {
        format!("Type to search scrollback ({})", search.scope.label())
    } else if search.scan.is_some() {
        "Scanning…".to_string()
    } else {
        format!("No matches for `{query}`")
    }
}

fn scrollback_search_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.is(KeyCode::Esc) {
            Some(Msg::CloseSearch)
        } else if matches!(key.code, KeyCode::Tab | KeyCode::BackTab) {
            Some(Msg::SearchCycleScope)
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            Some(Msg::SearchNext(false))
        } else if key.mods.ctrl && matches!(key.code, KeyCode::Char('p') | KeyCode::Char('P')) {
            Some(Msg::SearchNext(true))
        } else {
            None
        }
    })
}
