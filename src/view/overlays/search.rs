pub(crate) fn search_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let Some(search) = ctx.state.search.as_ref() else {
        return Text::new("").into();
    };
    let body = VStack::new()
        .height(Length::Auto)
        .child(scrollback_search_palette(ctx, search))
        .child(scrollback_search_hints(ctx, search));

    action_palette_modal_with_width(ctx, "Search scrollback", 90)
        .on_close(ctx.link().callback(|_| Msg::CloseSearch))
        .child(action_palette_frame(body))
        .key(search_input_key())
}

fn scrollback_search_hints(ctx: &Context<HyprmuxApp>, search: &ScrollbackSearchState) -> Element {
    let theme = &ctx.state.theme;
    hint_row()
        .child(hint_pill(theme, "next", "ctrl+n"))
        .child(hint_pill(theme, "previous", "ctrl+p"))
        .child(hint_pill(theme, search.scope.label(), "tab"))
        .into()
}

fn scrollback_search_palette(
    ctx: &Context<HyprmuxApp>,
    search: &ScrollbackSearchState,
) -> SearchPalette<usize> {
    let current = search.current;
    let query = search.input.text().trim();
    let entries = search
        .matches
        .iter()
        .enumerate()
        .map(|(index, matched)| {
            SearchEntry::item(search_match_label(matched), index)
                .description(search_match_description(matched))
                .active(index == current)
        })
        .collect::<Vec<_>>();

    let empty_text = if query.is_empty() {
        format!("Type to search scrollback ({})", search.scope.label())
    } else {
        format!("No matches for `{query}`")
    };

    shared_search_palette::<usize>(ctx, Length::Auto, true)
        .entries(entries)
        .placeholder("Search scrollback...")
        .initial_query(query.to_string())
        .preserve_groups(true)
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

fn search_match_label(matched: &ScrollbackMatch) -> String {
    let label = matched.text.trim();
    if label.is_empty() {
        "(blank line)".to_string()
    } else {
        label.to_string()
    }
}

fn search_match_description(matched: &ScrollbackMatch) -> String {
    format!(
        "pane {} · row {} · col {}",
        matched.pane,
        matched.line + 1,
        matched.start_col + 1
    )
}
