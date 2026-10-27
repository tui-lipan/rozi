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
    let detail = picker
        .selected
        .as_ref()
        .and_then(|selected| rows.iter().find(|row| &row.location == selected))
        .map(|row| {
            let (_, color, _) =
                crate::view::sidebar::agents::row_glyph(&row.status, row.finished_unseen, theme);
            super::common::detail_line(detail_spans(
                row,
                Style::new().fg(color),
                fg_only(&theme.muted),
            ))
        });
    let render_rows = Arc::new(rows);
    let name_style = fg_only(&theme.primary).bold();
    let agent_style = fg_only(&theme.muted);
    let location_style = fg_only(&theme.muted);
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
        // A run its client keeps in the background leans, as it does in the sidebar.
        let name_style = if row.background {
            name_style.italic()
        } else {
            name_style
        };
        let mut label = vec![Span::new(row.title()).style(name_style)];
        if row.task.is_some() {
            label.push(Span::new(format!(" · {}", row.agent)).style(agent_style));
        }
        let mut description = Vec::new();
        if let Some(note) = state_note(row) {
            description.push(Span::new(note).style(Style::new().fg(*color)));
        }
        if show_location {
            let place = crate::view::vocabulary::session_at(&row.session, row.host.as_deref());
            let place = if description.is_empty() {
                place
            } else {
                format!(" · {place}")
            };
            description.push(Span::new(place).style(location_style));
        }
        Some(
            ListItem::from_spans(label)
                .description_spans(description)
                // What and where win the row, so a long task starts right after a short name, but
                // keeps a third of it. One that fits stays right-aligned.
                .primary_truncate_description_first(false)
                .primary_min_label_width(Length::Percent(33))
                .primary_description_gap(2),
        )
    }))
    .detail(detail)
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

/// What the right side of a row says about its state, beyond the glyph: why it is waiting or
/// failed when it said, or a publisher's own status word, which no glyph can show. A canonical state
/// with nothing to add says nothing - the glyph and its color already do.
fn state_note(row: &crate::view::agents::GlobalAgentRow) -> Option<String> {
    use crate::session::protocol::ActivityState;
    let state = ActivityState::from_status(&row.status);
    match state {
        ActivityState::Blocked | ActivityState::Error => row.reason.clone(),
        ActivityState::Custom => Some(row.status.trim().to_string()),
        _ => None,
    }
}

/// The highlighted row in full: its state, how long it has held or what the run took, why, and
/// the project and branch it works in. Everything here is also searchable from the query field.
fn detail_spans(
    row: &crate::view::agents::GlobalAgentRow,
    state_style: Style,
    muted: Style,
) -> Vec<Span> {
    let rest = crate::view::vocabulary::fields(
        row.elapsed
            .map(crate::view::sidebar::agents::format_age)
            .into_iter()
            .chain(row.reason.clone())
            .chain(row.place()),
    );
    let mut spans = vec![Span::new(row.status_label()).style(state_style)];
    if !rest.is_empty() {
        spans.push(Span::new(format!(" · {rest}")).style(muted));
    }
    spans
}

/// Size from all rows so filtering does not make the palette jump horizontally.
fn picker_width(rows: &[crate::view::agents::GlobalAgentRow], show_location: bool) -> u16 {
    use unicode_width::UnicodeWidthStr;
    let widest = rows
        .iter()
        .map(|row| {
            let agent = row.task.as_ref().map_or(0, |_| row.agent.width() + 3);
            let note = state_note(row).map_or(0, |note| note.width());
            let location = if show_location {
                crate::view::vocabulary::session_at(&row.session, row.host.as_deref()).width()
                    + if note > 0 { 3 } else { 0 }
            } else {
                0
            };
            let right = note + location;
            row.title().width() + agent + if right > 0 { right + 2 } else { 0 }
        })
        .max()
        .unwrap_or(0);
    // Frame border, row padding, and the status gutter.
    (widest + 6).clamp(64, 140) as u16
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
            background: false,
        }
    }

    #[test]
    fn width_fits_task_note_and_location_in_terminal_columns_within_bounds() {
        assert_eq!(picker_width(&[], true), 64);
        assert_eq!(picker_width(&[row("")], true), 64);
        // 40 columns of task, ` · Codex`, a two-column gap, and `dev@workbox`.
        assert_eq!(picker_width(&[row(&"界".repeat(20))], true), 67);
        assert_eq!(picker_width(&[row(&"界".repeat(20))], false), 64);
        assert_eq!(picker_width(&[row(&"x".repeat(400))], true), 140);
    }

    /// The glyph says what state a row is in; the row adds only what the glyph cannot.
    #[test]
    fn a_row_notes_why_it_waits_or_failed_and_custom_words_but_not_plain_states() {
        let mut row = row("fix login");
        assert_eq!(
            state_note(&row),
            None,
            "working says nothing the spinner does not"
        );
        row.status = "blocked".into();
        assert_eq!(state_note(&row), None);
        row.reason = Some("Permission required".into());
        assert_eq!(state_note(&row).as_deref(), Some("Permission required"));
        row.status = "error".into();
        assert_eq!(state_note(&row).as_deref(), Some("Permission required"));
        row.status = "compacting".into();
        assert_eq!(state_note(&row).as_deref(), Some("compacting"));
    }

    /// Project, branch, reason, and time move to the highlighted row's detail line.
    #[test]
    fn the_detail_line_carries_what_the_row_leaves_out() {
        let mut row = row("fix login");
        row.reason = Some("Running tests".into());
        let text: String = detail_spans(&row, Style::new(), Style::new())
            .iter()
            .map(|span| span.content.to_string())
            .collect();
        assert_eq!(text, "Working · 4m00s · Running tests · api (fix-login)");
    }
}
