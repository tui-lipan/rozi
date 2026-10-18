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
            SearchEntry::Item(
                SearchItem::new(row.label(), row.location.clone())
                    .aliases(row.search_aliases(&ctx.state)),
            )
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
    let render_rows = Arc::new(rows);
    let name_style = fg_only(&theme.primary).bold();
    let detail_style = fg_only(&theme.muted);
    let agent_style = fg_only(&theme.muted);
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
    // Two agents can share a name and session (`Codex · dev` twice), which the shared label key
    // would fold into one row, so every refresh would put the cursor back on the first of them.
    .item_key(|item| Arc::from(format!("{:?}", item.value)))
    .render_item(Arc::new(move |item, _highlight| {
        let row = render_rows.iter().find(|row| row.location == item.value)?;
        let (_, color) = status_colors
            .iter()
            .find(|(location, _)| location == &item.value)?;
        let mut label = vec![Span::new(row.title()).style(name_style)];
        if row.task.is_some() {
            label.push(Span::new(format!(" · {}", row.agent)).style(agent_style));
        }
        let mut description = vec![Span::new(row.status_label()).style(Style::new().fg(*color))];
        if let Some(reason) = &row.reason {
            description.push(Span::new(format!(" · {reason}")).style(Style::new().fg(*color)));
        }
        if let Some(elapsed) = row.elapsed {
            description.push(
                Span::new(format!(
                    " · {}",
                    crate::view::sidebar::agents::format_age(elapsed)
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
                // The state and location win the row, so a long one starts right after a short
                // name, but a long name keeps a third of it. One that fits stays right-aligned.
                .primary_truncate_description_first(false)
                .primary_min_label_width(Length::Percent(33))
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
                crate::view::vocabulary::picker_circle_spinner_gutter(style)
            } else {
                crate::view::vocabulary::picker_marker_gutter(glyph, style)
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
        overlay = overlay.tabs(
            OverlayTabs::new(
                tabs.iter().map(|tab| tab.label(&ctx.state)).collect(),
                active,
                Msg::AgentPickerTab,
            )
            .with_all(),
        );
    }
    overlay.render(ctx)
}

/// Session and checkout context stay together, leaving the agent's work as the primary label.
fn row_context(row: &crate::view::agents::GlobalAgentRow, show_location: bool) -> String {
    if !show_location {
        return row.place().unwrap_or_default();
    }
    crate::view::vocabulary::fields([
        crate::view::vocabulary::session_at(&row.session, row.host.as_deref()),
        row.place().unwrap_or_default(),
    ])
}

/// Size from all rows so filtering does not make the palette jump horizontally.
fn picker_width(rows: &[crate::view::agents::GlobalAgentRow], show_location: bool) -> u16 {
    use unicode_width::UnicodeWidthStr;
    let widest = rows
        .iter()
        .map(|row| {
            let agent = row.task.as_ref().map_or(0, |_| row.agent.width() + 3);
            let status = row.status_label();
            let reason = row.reason.as_deref().map_or(0, |reason| reason.width() + 3);
            let elapsed = row.elapsed.map_or(0, |elapsed| {
                crate::view::sidebar::agents::format_age(elapsed).width() + 3
            });
            let context = row_context(row, show_location);
            row.title().width()
                + agent
                + reason
                + if context.is_empty() {
                    0
                } else {
                    context.width() + 3
                }
                + status.width()
                + elapsed
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

    fn row(task: &str) -> GlobalAgentRow {
        GlobalAgentRow {
            location: AgentLocation::Here { pane: 1, row: None },
            agent: "Codex".into(),
            task: Some(task.into()),
            host: Some("workbox".into()),
            session: "dev".into(),
            status: "working".into(),
            reason: None,
            finished_unseen: false,
            elapsed: Some(std::time::Duration::from_secs(240)),
            project: Some("api".into()),
            branch: Some("fix-login".into()),
            ordinal: None,
        }
    }

    #[test]
    fn width_fits_context_and_task_in_terminal_columns_within_bounds() {
        assert_eq!(picker_width(&[], true), 72);
        assert_eq!(picker_width(&[row("")], true), 72);
        assert_eq!(picker_width(&[row(&"界".repeat(20))], true), 103);
        assert_eq!(picker_width(&[row(&"界".repeat(20))], false), 89);
        assert_eq!(picker_width(&[row(&"x".repeat(400))], true), 160);
    }

    #[test]
    fn context_groups_host_session_and_checkout_and_hides_generated_session_names() {
        let mut row = row("fix login");
        assert_eq!(row_context(&row, true), "dev@workbox · api (fix-login)");
        assert_eq!(row_context(&row, false), "api (fix-login)");
        row.session = "eph-1234".into();
        row.host = None;
        assert_eq!(row_context(&row, true), "ephemeral · api (fix-login)");
        row.project = None;
        assert_eq!(row_context(&row, true), "ephemeral");
        assert_eq!(row_context(&row, false), "");
    }
}
