pub(crate) fn layout_picker_overlay(ctx: &Context<AppRoot>) -> Element {
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
    let selected = ctx
        .state
        .layout_picker
        .as_ref()
        .map(|picker| picker.selected);
    let selected_action = selected.unwrap_or_default();
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
                entry = entry.description(picker_description(description));
            }
            entry
        })
        .collect::<Vec<_>>();
    let actions = vec![
        OverlayAction::new("enter", "switch", Msg::SelectLayout(selected_action), true).hint_only(),
        OverlayAction::new("ctrl-f", "set default", Msg::LayoutPickerSetDefault, true),
    ];

    OverlayPalette::new(
        "Choose layout",
        layout_picker_key(),
        Msg::CloseLayoutPicker,
        52,
    )
    .entries(entries)
    .actions(actions)
    .placeholder("Search layouts…")
    .preserve_groups(false)
    .selected(selected)
    .initial_query(
        ctx.state
            .layout_picker
            .as_ref()
            .map(|picker| picker.query.clone())
            .unwrap_or_default(),
    )
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::LayoutPickerQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::LayoutPickerSelect(event.item.value)),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<usize>| Msg::SelectLayout(event.item.value)),
    )
    .render(ctx)
}
