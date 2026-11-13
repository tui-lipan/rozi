use super::*;

use crate::state::{AgentLocation, AgentPickerTab};
use crate::view::agents::GlobalAgentRow;

/// The global Agents view: every agent this client knows about, wherever it is running, ordered by
/// what wants attention and sectioned by it. `Enter` lands on the one under the cursor — a focus change for a pane in
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
    let entries = grouped_entries(&rows, |entry| {
        SearchEntry::Item(
            SearchItem::new(entry.row.label(), entry.row.location.clone())
                .aliases(entry.row.search_aliases(&ctx.state)),
        )
        .description(picker_description(entry.row.description()))
    });
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
        WIDTH,
    )
    .entries(entries)
    // Rows stay under their state while searching, in the same order as without a query.
    .preserve_groups(true)
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

/// One width whatever the tab, query, or rows, so switching tabs never moves the frame. Wide enough
/// for a task and its whole right side on a usual screen; a narrower terminal bounds it further.
const WIDTH: u16 = 120;

/// Where a row lists, by what it wants from the user.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Section {
    NeedsYou,
    Working,
    Done,
    Idle,
}

impl Section {
    /// Follows [`GlobalAgentRow::rank`], which already orders the rows, so each section is one run
    /// of them.
    fn of(row: &GlobalAgentRow) -> Self {
        use crate::session::protocol::ActivityState;
        if row.needs_attention() {
            return Self::NeedsYou;
        }
        match ActivityState::from_status(&row.status) {
            ActivityState::Working | ActivityState::Custom => Self::Working,
            ActivityState::Done => Self::Done,
            _ => Self::Idle,
        }
    }

    fn title(self) -> &'static str {
        use crate::view::vocabulary::{AGENTS_DONE, AGENTS_IDLE, AGENTS_NEED_YOU, AGENTS_WORKING};
        match self {
            Self::NeedsYou => AGENTS_NEED_YOU,
            Self::Working => AGENTS_WORKING,
            Self::Done => AGENTS_DONE,
            Self::Idle => AGENTS_IDLE,
        }
    }
}

/// The rows under a header for each section that has any, in row order.
fn grouped_entries<T>(
    rows: &[PickerRow],
    entry: impl Fn(&PickerRow) -> SearchEntry<T>,
) -> Vec<SearchEntry<T>> {
    let mut groups: Vec<(&'static str, Vec<SearchEntry<T>>)> = Vec::new();
    for row in rows {
        let title = Section::of(&row.row).title();
        match groups.last_mut() {
            Some((last, items)) if *last == title => items.push(entry(row)),
            _ => groups.push((title, vec![entry(row)])),
        }
    }
    crate::view::search_entries_with_groups(groups)
}

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
    /// The pull request of the branch it works on, with the status that colours it.
    PullRequest(String, crate::state::AgentPullRequest),
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
            Self::State(text)
            | Self::PullRequest(text, _)
            | Self::Session(text)
            | Self::Checkout(text) => text,
            Self::Current => crate::view::vocabulary::CURRENT,
        }
    }
}

/// The right side of a row, most important first, because it gives way from its end: the state
/// and its time, why it is in that state, the pull request of its branch, `Current` on the agent on
/// screen, the session, and last the checkout, which a row short of room loses first.
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
    fields.extend(row.pull_request.as_ref().map(|pull_request| {
        Field::PullRequest(
            crate::view::vocabulary::pull_request_badge(pull_request),
            pull_request.clone(),
        )
    }));
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
            Field::PullRequest(_, ref pull_request) => {
                crate::view::vocabulary::agent_pull_request_style(theme, pull_request)
            }
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
            pull_request_source: None,
            pull_request: None,
        }
    }

    fn entry(row: GlobalAgentRow, current: bool) -> PickerRow {
        PickerRow { row, current }
    }

    /// A finished run nobody has seen still wants the user, and a publisher's own word is still a
    /// run in progress, so neither lists with the quiet ones.
    #[test]
    fn rows_list_under_the_section_for_what_they_want_from_the_user() {
        let with = |status: &str, unseen: bool| {
            let mut row = row(status);
            row.status = status.into();
            row.finished_unseen = unseen;
            entry(row, false)
        };
        let mut rows = vec![
            with("blocked", false),
            with("done", true),
            with("compacting", false),
            with("working", false),
            with("done", false),
            with("idle", false),
        ];
        rows.sort_by_key(|entry| entry.row.rank());
        let entries = grouped_entries(&rows, |entry| {
            SearchEntry::item(entry.row.status.clone(), ())
        });
        let listed = entries
            .iter()
            .filter_map(|entry| match entry {
                SearchEntry::Header(title) => Some(format!("# {title}")),
                SearchEntry::Item(item) => Some(item.label.to_string()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            listed,
            [
                "# Needs you",
                "blocked",
                "done",
                "# Working",
                "compacting",
                "working",
                "# Done",
                "done",
                "# Idle",
                "idle"
            ]
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
