/// The global Agents view: every agent this client knows about, wherever it is running, ordered by
/// what wants attention. `Enter` lands on the one under the cursor — a focus change for a pane in
/// the session on screen, and an attach followed by a focus change for anything else.
///
/// Its rows are a pure projection of `State` (see [`crate::view::agents::global_agent_rows`]), so
/// there is nothing here to keep in step with a poll: the list is whatever the last snapshot and
/// the live panes currently say.
pub(crate) fn agent_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.agent_picker.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let rows = crate::view::agents::global_agent_rows(&ctx.state);
    let entries = rows
        .iter()
        .map(|row| {
            SearchEntry::item(row.label(), row.location.clone())
                .description(picker_description(row.description()))
        })
        .collect::<Vec<_>>();
    let selected = picker
        .selected
        .as_ref()
        .and_then(|selected| rows.iter().position(|row| &row.location == selected));
    let empty_text = if picker.input.text().trim().is_empty() {
        // Says which machines were asked, not merely that the answer was empty: nothing running on
        // a client connected to nowhere is not news, and nothing running across four connected
        // hosts is.
        match ctx.state.host_agents.len() {
            0 => "No agents running here".to_string(),
            1 => "No agents running here or on the connected host".to_string(),
            count => format!("No agents running here or on {count} connected hosts"),
        }
    } else {
        format!("No agents match `{}`", picker.input.text().trim())
    };
    // Opening a row on another machine attaches to its session on the way, which is a larger step
    // than a focus change and is worth saying before it is taken.
    let selected_is_remote = picker
        .selected
        .as_ref()
        .is_some_and(|location| matches!(location, crate::state::AgentLocation::Elsewhere { .. }));
    let actions = vec![
        OverlayAction::new(
            "enter",
            if selected_is_remote {
                "open there"
            } else {
                "go to"
            },
            picker
                .selected
                .clone()
                .map(Msg::AgentPickerActivate)
                .unwrap_or(Msg::CloseAgentPicker),
            picker.selected.is_some(),
        )
        .hint_only(),
    ];
    let fallback = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::CloseAgentPicker));
    // Resolved up front so the gutter closure carries colors rather than the theme and the rows.
    let gutters = rows
        .iter()
        .map(|row| {
            let (glyph, color, working) =
                crate::view::sidebar::agents::row_glyph(&row.status, row.finished_unseen, theme);
            (row.location.clone(), glyph, color, working)
        })
        .collect::<Vec<_>>();
    OverlayPalette::new(
        "Agents",
        crate::view::agent_picker_key(),
        Msg::CloseAgentPicker,
        64,
    )
    .entries(entries)
    .actions(actions)
    .placeholder("Search agents...")
    .initial_query(picker.input.text().to_string())
    .selected(selected)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .item_gutter(Arc::new(
        move |item: &SearchItem<crate::state::AgentLocation>, _hl| {
            let (_, glyph, color, working) =
                gutters.iter().find(|(location, ..)| location == &item.value)?;
            let style = Style::new().fg(*color);
            Some(if *working {
                crate::view::session_status::picker_circle_spinner_gutter(style)
            } else {
                crate::view::session_status::picker_marker_gutter(glyph, style)
            })
        },
    ))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::AgentPickerQueryChanged(query.to_string())),
    )
    .on_select(
        ctx.link()
            .callback(|event: SearchEvent<crate::state::AgentLocation>| {
                Msg::AgentPickerSelect(event.item.value.clone())
            }),
    )
    .on_activate(
        ctx.link()
            .callback(|event: SearchEvent<crate::state::AgentLocation>| {
                Msg::AgentPickerActivate(event.item.value.clone())
            }),
    )
    .render(ctx)
}
