pub(crate) fn layout_picker_overlay(ctx: &Context<HyprmuxApp>) -> Element {
    let body = VStack::new()
        .height(Length::Auto)
        .child(layout_picker_palette(ctx))
        .child(layout_picker_hints(ctx));

    action_palette(
        ctx,
        "Choose layout",
        layout_picker_key(),
        Msg::CloseLayoutPicker,
        body,
        52,
    )
}

fn layout_picker_hints(ctx: &Context<HyprmuxApp>) -> Element {
    let theme = &ctx.state.theme;
    hint_row()
        .child(hint_pill(theme, "switch", "enter"))
        .child(hint_pill(theme, "set default", "ctrl+f"))
        .into()
}

fn layout_picker_palette(ctx: &Context<HyprmuxApp>) -> SearchPalette<usize> {
    let workspace_index = ctx.state.current().active_workspace;
    // The real applied layout is the one the picker opened on and will revert to on cancel — not the
    // live-previewed layout under the highlight, which changes as the user browses. `current`
    // therefore tracks `original`, so the "current" badge stays on the row we return to.
    let current = ctx
        .state
        .layout_picker
        .as_ref()
        .map(|picker| picker.original)
        .unwrap_or_else(|| ctx.state.current().workspaces[workspace_index].layout_kind);
    let default = ctx.state.config.layout.default;
    let selected = ctx.state.layout_picker.as_ref().map(|picker| picker.selected);

    let entries = crate::state::LayoutKind::all()
        .iter()
        .enumerate()
        .map(|(index, kind)| {
            let mut entry = SearchEntry::item(kind.label(), index);
            let description = match (*kind == current, *kind == default) {
                (true, true) => "current  default",
                (true, false) => "current",
                (false, true) => "default",
                (false, false) => "",
            };
            if !description.is_empty() {
                entry = entry.description(ItemDescription::new().right(description));
            }
            entry
        })
        .collect::<Vec<_>>();

    shared_search_palette::<usize>(ctx, Length::Auto, false)
        .entries(entries)
        .placeholder("Search layouts…")
        .preserve_groups(false)
        .initial_selected_item_index(selected)
        .sync_selection(true)
        .input_key_interceptor(layout_picker_key_interceptor(ctx))
        .on_select(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::LayoutPickerSelect(event.item.value)),
        )
        .on_activate(
            ctx.link()
                .callback(|event: SearchEvent<usize>| Msg::SelectLayout(event.item.value)),
        )
}

fn layout_picker_key_interceptor(ctx: &Context<HyprmuxApp>) -> KeyHandler {
    ctx.link().key_handler(|key| {
        if key.mods.ctrl && matches!(key.code, KeyCode::Char('f') | KeyCode::Char('F')) {
            Some(Msg::LayoutPickerSetDefault)
        } else {
            None
        }
    })
}
