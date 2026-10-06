use super::*;

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
    let tabs = crate::ops::agents::picker_tabs(&ctx.state);
    let active = tabs.iter().position(|tab| tab == &picker.tab).unwrap_or(0);
    let show_location = picker.tab == crate::state::AgentPickerTab::All;
    let rows = rows
        .into_iter()
        .filter(|row| row.in_tab(&ctx.state, &picker.tab))
        .collect::<Vec<_>>();
    let width = picker_width(&rows, show_location);
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
    let empty_text = if picker.input.text().trim().is_empty() && !show_location {
        "No agents in this session".to_string()
    } else if picker.input.text().trim().is_empty() {
        // Says which machines were asked, not merely that the answer was empty: nothing running on
        // a client connected to nowhere is not news, and nothing running across four connected
        // hosts is.
        match ctx.state.remote.agents.len() {
            0 => "No agents running here".to_string(),
            1 => "No agents running here or on the connected host".to_string(),
            count => format!("No agents running here or on {count} connected hosts"),
        }
    } else {
        format!("No agents match `{}`", picker.input.text().trim())
    };
    // Opening a row in another session attaches on the way, which is a larger step than a focus
    // change and is worth saying before it is taken.
    let selected_is_other_session = picker.selected.as_ref().is_some_and(|location| {
        matches!(location, crate::state::AgentLocation::OtherSession { .. })
    });
    let actions = vec![
        OverlayAction::new(
            "enter",
            if selected_is_other_session {
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
    let description_width = width.min(ctx.viewport().w).saturating_sub(8) * 2 / 3;
    let render_rows = Arc::new(rows);
    let name_style = fg_only(&theme.primary).bold();
    let detail_style = fg_only(&theme.muted);
    let activity_style = fg_only(&theme.primary);
    let status_colors = gutters
        .iter()
        .map(|(location, _, color, _)| (location.clone(), *color))
        .collect::<Vec<_>>();
    let mut overlay = OverlayPalette::new(
        "Agents",
        crate::view::agent_picker_key(),
        Msg::CloseAgentPicker,
        width,
    )
    .entries(entries)
    .render_item(Arc::new(move |item, _highlight| {
        let row = render_rows.iter().find(|row| row.location == item.value)?;
        let (_, color) = status_colors
            .iter()
            .find(|(location, _)| location == &item.value)?;
        let mut label = vec![Span::new(row.agent.clone()).style(name_style)];
        if let Some(activity) = &row.activity {
            label.push(Span::new(format!(": {activity}")).style(activity_style));
        }
        let status =
            crate::view::sidebar::agents::row_status_label(&row.status, row.finished_unseen);
        let mut description = vec![Span::new(status).style(Style::new().fg(*color))];
        if let Some(age) = row.age {
            description.push(
                Span::new(format!(
                    " · {}",
                    crate::view::sidebar::agents::format_age(age)
                ))
                .style(detail_style),
            );
        }
        let context = row_context(row, show_location);
        if !context.is_empty() {
            description.push(Span::new(format!(" · {context}")).style(detail_style));
        }
        Some(
            ListItem::from_spans(label)
                .description_spans(description)
                .primary_truncate_description_first(false)
                .primary_max_description_width(description_width)
                .primary_description_gap(2),
        )
    }))
    .actions(actions)
    .placeholder("Search agents…")
    .initial_query(picker.input.text().to_string())
    .selected(selected)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .item_gutter(Arc::new(
        move |item: &SearchItem<crate::state::AgentLocation>, _hl| {
            let (_, glyph, color, working) = gutters
                .iter()
                .find(|(location, ..)| location == &item.value)?;
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
    );
    if tabs.len() > 1 {
        overlay = overlay.tabs(OverlayTabs::new(
            tabs.iter()
                .map(crate::state::AgentPickerTab::label)
                .collect(),
            active,
            Msg::AgentPickerTab,
        ));
    }
    overlay.render(ctx)
}

/// Session and checkout context stay together, leaving the agent's activity as the primary label.
fn row_context(row: &crate::view::agents::GlobalAgentRow, show_location: bool) -> String {
    if !show_location {
        return row.place.clone().unwrap_or_default();
    }
    let session = if crate::state::is_ephemeral_session_name(&row.session) {
        "ephemeral"
    } else {
        row.session.as_str()
    };
    let location = match &row.host {
        Some(host) => format!("{host}/{session}"),
        None => session.to_string(),
    };
    match &row.place {
        Some(place) => format!("{location} · {place}"),
        None => location,
    }
}

/// Size from all rows so filtering does not make the palette jump horizontally.
fn picker_width(rows: &[crate::view::agents::GlobalAgentRow], show_location: bool) -> u16 {
    use unicode_width::UnicodeWidthStr;
    let widest = rows
        .iter()
        .map(|row| {
            let activity = row.activity.as_deref().map_or(0, |text| text.width() + 2);
            let status =
                crate::view::sidebar::agents::row_status_label(&row.status, row.finished_unseen);
            let age = row.age.map_or(0, |age| {
                crate::view::sidebar::agents::format_age(age).width() + 3
            });
            let context = row_context(row, show_location);
            row.agent.width()
                + activity
                + if context.is_empty() {
                    0
                } else {
                    context.width() + 3
                }
                + status.width()
                + age
        })
        .max()
        .unwrap_or(0);
    // Frame border, row padding, status gutter, and label/description gap.
    (widest + 8).clamp(72, 160) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AgentLocation;
    use crate::view::agents::GlobalAgentRow;

    fn row(activity: &str) -> GlobalAgentRow {
        GlobalAgentRow {
            location: AgentLocation::Here { pane: 1, row: None },
            agent: "Codex".into(),
            host: Some("workbox".into()),
            session: "dev".into(),
            status: "working".into(),
            finished_unseen: false,
            age: Some(std::time::Duration::from_secs(240)),
            activity: Some(activity.into()),
            place: Some("api (fix-login)".into()),
        }
    }

    #[test]
    fn width_fits_context_and_activity_in_terminal_columns_within_bounds() {
        assert_eq!(picker_width(&[], true), 72);
        assert_eq!(picker_width(&[row("")], true), 72);
        assert_eq!(picker_width(&[row(&"界".repeat(20))], true), 102);
        assert_eq!(picker_width(&[row(&"界".repeat(20))], false), 88);
        assert_eq!(picker_width(&[row(&"x".repeat(400))], true), 160);
    }

    #[test]
    fn context_groups_host_session_and_checkout_and_hides_generated_session_names() {
        let mut row = row("fix login");
        assert_eq!(row_context(&row, true), "workbox/dev · api (fix-login)");
        assert_eq!(row_context(&row, false), "api (fix-login)");
        row.session = "eph-1234".into();
        row.host = None;
        assert_eq!(row_context(&row, true), "ephemeral · api (fix-login)");
        row.place = None;
        assert_eq!(row_context(&row, true), "ephemeral");
        assert_eq!(row_context(&row, false), "");
    }
}
