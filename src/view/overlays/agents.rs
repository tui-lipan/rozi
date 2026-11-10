use super::*;

use crate::state::{AgentLocation, AgentPickerTab};
use crate::view::agents::GlobalAgentRow;

/// The global Agents view: every agent this client knows about, wherever it is running, ordered by
/// what wants attention. `Enter` lands on the one under the cursor — a focus change for a pane in
/// the session on screen, and an attach followed by a focus change for anything else.
///
/// Its rows are a pure projection of `State` (see [`crate::view::agents::global_agent_rows`]), so
/// there is nothing here to keep in step with a poll: the list is whatever the last snapshot and
/// the live panes currently say.
///
/// Everything a row says is on the row, as in every other picker. The frame header names the
/// session on screen, and the right side carries the rest in the order it gives way.
pub(crate) fn agent_picker_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(picker) = ctx.state.agent_picker.as_ref() else {
        return Text::new("").into();
    };
    let theme = &ctx.state.theme;
    let tabs = crate::ops::agents::picker_tabs(&ctx.state);
    let active = tabs.iter().position(|tab| tab == &picker.tab).unwrap_or(0);
    let rows = crate::view::agents::global_agent_rows(&ctx.state)
        .into_iter()
        .filter(|row| row.in_tab(&ctx.state, &picker.tab))
        .map(|row| PickerRow {
            current: row.is_on_screen(&ctx.state),
            row,
        })
        .collect::<Vec<_>>();
    // A session tab says where its rows run, and so does the header when every row is in the
    // session on screen. Only rows from elsewhere need to name theirs.
    let show_session =
        picker.tab == AgentPickerTab::All && rows.iter().any(|entry| !entry.row.is_here());
    let width = picker_width(&rows, show_session);
    let entries = rows
        .iter()
        .map(|entry| {
            SearchEntry::Item(
                SearchItem::new(entry.row.label(), entry.row.location.clone())
                    .aliases(entry.row.search_aliases(&ctx.state)),
            )
            .description(picker_description(entry.row.description()))
        })
        .collect::<Vec<_>>();
    let selected_row = picker.selected.as_ref().and_then(|selected| {
        rows.iter()
            .position(|entry| &entry.row.location == selected)
    });
    let query = picker.input.text().trim();
    let empty_text = if !query.is_empty() {
        format!("No agents match `{query}`")
    } else if picker.tab != AgentPickerTab::All {
        "No agents in this session".to_string()
    } else {
        // Says which machines were asked, not merely that the answer was empty: nothing running on
        // a client connected to nowhere is not news, and nothing running across four connected
        // hosts is.
        match ctx.state.remote.agents.len() {
            0 => "No agents running here".to_string(),
            1 => "No agents running here or on the connected host".to_string(),
            count => format!("No agents running here or on {count} connected hosts"),
        }
    };
    // Opening a row in another session attaches on the way, which is a larger step than a focus
    // change and is worth saying before it is taken. The agent on screen offers nothing: Enter
    // there only closes the picker, as on the Sessions picker's current session.
    let selected_entry = selected_row.map(|index| &rows[index]);
    let selected_is_other_session = selected_entry
        .is_some_and(|entry| matches!(entry.row.location, AgentLocation::OtherSession { .. }));
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
            selected_entry.is_some_and(|entry| !entry.current),
        )
        .hint_only(),
    ];
    let fallback = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::CloseAgentPicker));
    // Resolved up front so the closures carry spans and colors rather than the theme and the rows.
    let rendered = Arc::new(
        rows.iter()
            .map(|entry| RenderedRow::new(entry, show_session, theme))
            .collect::<Vec<_>>(),
    );
    let gutters = Arc::clone(&rendered);
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
        let row = rendered.iter().find(|row| row.location == item.value)?;
        Some(
            ListItem::from_spans(row.label.clone())
                .description_spans(row.description.clone())
                // What the agent works on wins the row: the right side gives way first, from its
                // end, until the label is down to its share. One that fits stays right-aligned,
                // and one cut down to nothing is hidden rather than drawn as a lone `…`.
                .primary_truncate_description_first(false)
                .primary_min_label_width(Length::Percent(LABEL_SHARE))
                .primary_description_gap(2),
        )
    }))
    .actions(actions)
    .placeholder("Search agents…")
    .initial_query(picker.input.text().to_string())
    .selected(selected_row)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .item_gutter(Arc::new(move |item: &SearchItem<AgentLocation>, _hl| {
        let row = gutters.iter().find(|row| row.location == item.value)?;
        let style = Style::new().fg(row.color);
        Some(if row.working {
            crate::view::vocabulary::picker_circle_spinner_gutter(style)
        } else {
            crate::view::vocabulary::picker_marker_gutter(&row.glyph, style)
        })
    }))
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::AgentPickerQueryChanged(query.to_string())),
    )
    .on_select(ctx.link().callback(|event: SearchEvent<AgentLocation>| {
        Msg::AgentPickerSelect(event.item.value.clone())
    }))
    .on_activate(ctx.link().callback(|event: SearchEvent<AgentLocation>| {
        Msg::AgentPickerActivate(event.item.value.clone())
    }));
    if let Some(session) = session_on_screen(&ctx.state) {
        overlay = overlay.header_right(session);
    }
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

/// The share of a row its label keeps when both sides are long. The task is what tells one run
/// from another, so it keeps more of the row than the facts beside it.
const LABEL_SHARE: u16 = 40;

/// The columns a row needs besides its label and right side: frame border, row padding, the
/// status gutter, and the gap between the two sides.
const ROW_CHROME: usize = 8;

const MIN_WIDTH: usize = 64;

/// Rows widen the picker up to this, so a long task keeps its whole right side on a wide screen.
/// A narrower terminal bounds it further.
const MAX_WIDTH: usize = 180;

/// One row and whether it is the agent on screen.
struct PickerRow {
    row: GlobalAgentRow,
    current: bool,
}

/// What the row renderer and gutter draw for one row.
struct RenderedRow {
    location: AgentLocation,
    label: Vec<Span>,
    description: Vec<Span>,
    glyph: String,
    color: Color,
    working: bool,
}

impl RenderedRow {
    fn new(entry: &PickerRow, show_session: bool, theme: &Theme) -> Self {
        let row = &entry.row;
        let (glyph, color, working) =
            crate::view::sidebar::agents::row_glyph(&row.status, row.finished_unseen, theme);
        Self {
            location: row.location.clone(),
            label: label_spans(row, theme),
            description: description_spans(entry, show_session, color, theme),
            glyph,
            color,
            working,
        }
    }
}

/// The session on screen as its tab names it, `dev` or `dev@workbox`, for the frame header. The
/// launcher has none.
fn session_on_screen(state: &crate::state::State) -> Option<String> {
    let current = state.current();
    let session = current.session_name.clone()?;
    Some(
        AgentPickerTab::Session {
            target: current.remote_target.clone(),
            session,
        }
        .label(state),
    )
}

/// The row's name: what the agent works on, then which agent it is when the work names the row.
/// A run its client keeps in the background leans, as it does in the sidebar.
fn label_spans(row: &GlobalAgentRow, theme: &Theme) -> Vec<Span> {
    let name_style = fg_only(&theme.primary).bold();
    let name_style = if row.background {
        name_style.italic()
    } else {
        name_style
    };
    let mut label = vec![Span::new(row.title()).style(name_style)];
    if row.work().is_some() {
        label.push(Span::new(format!(" · {}", row.agent)).style(fg_only(&theme.muted)));
    }
    label
}

/// One fact on the right side of a row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Field {
    /// Why the agent is in its state, and for how long.
    State(String),
    /// The agent on screen.
    Current,
    /// Where it runs, for a row outside the session on screen.
    Session(String),
    /// The checkout it works in.
    Checkout(String),
}

impl Field {
    fn text(&self) -> &str {
        match self {
            Self::State(text) | Self::Session(text) | Self::Checkout(text) => text,
            Self::Current => crate::view::vocabulary::CURRENT,
        }
    }
}

/// The right side of a row, most important first, because it gives way from its end: the state
/// and its time, why it is in that state, `Current` on the agent on screen, the session, and last
/// the checkout, which a row short of room loses first.
///
/// A time is never shown bare: it leads with the word for the state it measures, `Working 14s`,
/// as the sidebar's Agents tab writes it. A row with no time leaves the state to its glyph.
fn fields(entry: &PickerRow, show_session: bool) -> Vec<Field> {
    let row = &entry.row;
    let lead = row.elapsed.map(|elapsed| {
        format!(
            "{} {}",
            row.status_label(),
            crate::view::sidebar::agents::format_age(elapsed)
        )
    });
    // A custom state is its own word, which the lead already says.
    let custom = crate::session::protocol::ActivityState::from_status(&row.status)
        == crate::session::protocol::ActivityState::Custom;
    let note = state_note(row).filter(|_| lead.is_none() || !custom);
    let mut fields: Vec<Field> = lead.into_iter().chain(note).map(Field::State).collect();
    // The header names the session on screen, so the agent on screen says it is current in place
    // of that name.
    if entry.current {
        fields.push(Field::Current);
    } else if show_session {
        fields.push(Field::Session(crate::view::vocabulary::session_at(
            &row.session,
            row.host.as_deref(),
        )));
    }
    fields.extend(row.place().map(Field::Checkout));
    fields
}

/// The right side as spans: the state and its time in the state's colour, `Current` in the accent
/// colour - not the green the Sessions picker gives it, which here means a finished run - and the
/// places muted.
fn description_spans(
    entry: &PickerRow,
    show_session: bool,
    state_color: Color,
    theme: &Theme,
) -> Vec<Span> {
    let muted = fg_only(&theme.muted);
    let mut spans = Vec::new();
    for field in fields(entry, show_session) {
        if !spans.is_empty() {
            spans.push(Span::new(crate::view::vocabulary::SEPARATOR).style(muted));
        }
        let style = match field {
            Field::State(_) => Style::new().fg(state_color),
            Field::Current => fg_only(&theme.accent),
            Field::Session(_) | Field::Checkout(_) => muted,
        };
        spans.push(Span::new(field.text().to_string()).style(style));
    }
    spans
}

/// What the right side of a row says about its state, beyond the glyph: why it is waiting or
/// failed when it said, or a publisher's own status word, which no glyph can show. A canonical state
/// with nothing to add says nothing - the glyph and its color already do.
fn state_note(row: &GlobalAgentRow) -> Option<String> {
    use crate::session::protocol::ActivityState;
    let state = ActivityState::from_status(&row.status);
    match state {
        ActivityState::Blocked | ActivityState::Error => row.reason.clone(),
        ActivityState::Custom => Some(row.status.trim().to_string()),
        _ => None,
    }
}

/// Wide enough for the widest row's label and right side together, within bounds. Sized from all
/// rows so filtering does not make the palette jump horizontally.
fn picker_width(rows: &[PickerRow], show_session: bool) -> u16 {
    use unicode_width::UnicodeWidthStr;
    let separator = crate::view::vocabulary::SEPARATOR.width();
    let widest = rows
        .iter()
        .map(|entry| {
            let row = &entry.row;
            let label = row.title().width() + row.work().map_or(0, |_| row.agent.width() + 3);
            let fields = fields(entry, show_session);
            let right = fields
                .iter()
                .map(|field| field.text().width())
                .sum::<usize>()
                + separator * fields.len().saturating_sub(1);
            label + right
        })
        .max()
        .unwrap_or(0);
    (widest + ROW_CHROME).clamp(MIN_WIDTH, MAX_WIDTH) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(task: &str) -> GlobalAgentRow {
        GlobalAgentRow {
            location: AgentLocation::Here { pane: 1, row: None },
            agent_id: "codex".into(),
            checkout: None,
            agent: "Codex".into(),
            task: Some(task.into()),
            screen_title: None,
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

    fn entry(row: GlobalAgentRow, current: bool) -> PickerRow {
        PickerRow { row, current }
    }

    #[test]
    fn width_fits_task_and_right_side_in_terminal_columns_within_bounds() {
        assert_eq!(picker_width(&[], true), 64);
        assert_eq!(picker_width(&[entry(row(""), false)], true), 64);
        // 40 columns of task and ` · Codex` beside `Working 4m00s · dev@workbox · api (fix-login)`.
        let wide = row(&"界".repeat(20));
        assert_eq!(
            picker_width(&[entry(wide.clone(), false)], true),
            48 + 45 + 8
        );
        // `Current` stands in for the session, and without the session column there is none.
        assert_eq!(
            picker_width(&[entry(wide.clone(), true)], true),
            48 + 41 + 8
        );
        assert_eq!(picker_width(&[entry(wide, false)], false), 48 + 31 + 8);
        assert_eq!(
            picker_width(&[entry(row(&"x".repeat(400)), false)], true),
            180
        );
    }

    /// The right side reads most important first, so cutting it from the end loses the checkout,
    /// then the session, before what the agent is doing.
    #[test]
    fn the_right_side_leads_with_the_state_and_ends_with_the_checkout() {
        let mut blocked = row("fix login");
        blocked.status = "blocked".into();
        blocked.reason = Some("Permission required".into());
        let text = |entry: &PickerRow, show_session| {
            fields(entry, show_session)
                .iter()
                .map(Field::text)
                .collect::<Vec<_>>()
                .join(crate::view::vocabulary::SEPARATOR)
        };
        assert_eq!(
            text(&entry(blocked.clone(), false), true),
            "Blocked 4m00s · Permission required · dev@workbox · api (fix-login)"
        );
        assert_eq!(
            text(&entry(blocked.clone(), true), true),
            "Blocked 4m00s · Permission required · Current · api (fix-login)"
        );
        blocked.elapsed = None;
        blocked.project = None;
        assert_eq!(text(&entry(blocked, false), false), "Permission required");

        // A custom state names itself once, beside its time.
        let mut custom = row("fix login");
        custom.status = "compacting".into();
        custom.project = None;
        assert_eq!(
            text(&entry(custom.clone(), false), false),
            "compacting 4m00s"
        );
        custom.elapsed = None;
        assert_eq!(text(&entry(custom, false), false), "compacting");
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
}
