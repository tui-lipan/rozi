use super::*;

use crate::control::{AgentRequestState, AgentRequestSummary};
use crate::view::agent_requests::{
    ChainEvent, agent_name, chain_events, requests, short_duration, state_word, threaded,
};
use crate::view::vocabulary::SEPARATOR;
use crate::view::widget_keys::{agent_conversations_chain_key, agent_conversations_key};

const WIDTH: u16 = 110;
/// Modal border and the hint footer around the diagram.
const CHAIN_CHROME_ROWS: u16 = 4;

/// The requests between the session's agents: a list grouped by whether they still wait, and one
/// chain of them drawn as a sequence diagram.
///
/// Requests arrive from the session server without their prompts or answers, so this shows who
/// asks whom and how far each got, never what was said.
pub(crate) fn agent_conversations_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(view) = ctx.state.agent_conversations.as_ref() else {
        return Text::new("").into();
    };
    match view.chain.as_deref() {
        Some(root) => chain_overlay(ctx, root),
        None => list_overlay(ctx, view.selected.as_deref(), view.input.text()),
    }
}

/// Now, in the server's clock, for ages and durations.
fn now_ms() -> u64 {
    crate::runtime_metrics::unix_time_millis()
}

fn list_overlay(ctx: &Context<AppRoot>, selected: Option<&str>, query: &str) -> Element {
    let theme = &ctx.state.theme;
    let all = requests(&ctx.state);
    let rows = threaded(all);
    let now = now_ms();
    // A chain lists under Waiting while any request in it waits, so it is never split in two.
    let waiting_roots: Vec<&str> = all
        .iter()
        .filter(|request| !request.state.is_resolved())
        .map(|request| request.root.as_str())
        .collect();
    let rendered: Arc<Vec<RenderedRow>> = Arc::new(
        rows.iter()
            .map(|(request, depth)| {
                let waiting = waiting_roots.contains(&request.root.as_str());
                RenderedRow::new(&ctx.state, request, *depth, waiting, now, theme)
            })
            .collect(),
    );
    let mut groups: Vec<(&'static str, Vec<SearchEntry<String>>)> = Vec::new();
    for row in rendered.iter() {
        let title = if row.waiting { "Waiting" } else { "Settled" };
        let entry = SearchEntry::Item(
            SearchItem::new(row.search.clone(), row.id.clone()).aliases(vec![row.root.clone()]),
        );
        match groups.iter_mut().find(|(group, _)| *group == title) {
            Some((_, items)) => items.push(entry),
            None => groups.push((title, vec![entry])),
        }
    }
    // Waiting first, whatever order the chains came in.
    groups.sort_by_key(|(title, _)| *title != "Waiting");
    let entries = crate::view::search_entries_with_groups(groups);
    let ordered: Vec<&RenderedRow> = {
        let mut ordered: Vec<&RenderedRow> = rendered.iter().filter(|row| row.waiting).collect();
        ordered.extend(rendered.iter().filter(|row| !row.waiting));
        ordered
    };
    let selected_row = selected.and_then(|id| ordered.iter().position(|row| row.id == id));
    let selected_id = selected_row.map(|index| ordered[index].id.clone());
    let actions = vec![
        OverlayAction::new(
            "enter",
            "show chain",
            selected_id
                .clone()
                .map(Msg::AgentConversationsOpenChain)
                .unwrap_or(Msg::CloseAgentConversations),
            selected_id.is_some(),
        )
        .hint_only(),
        OverlayAction::new(
            "ctrl-g",
            "go to",
            selected_id
                .clone()
                .map(Msg::AgentConversationsGoTo)
                .unwrap_or(Msg::CloseAgentConversations),
            selected_id.is_some(),
        ),
    ];
    let empty_text = if query.trim().is_empty() {
        "No requests between agents in this session".to_string()
    } else {
        format!("No requests match `{}`", query.trim())
    };
    let fallback = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::CloseAgentConversations));
    let items = Arc::clone(&rendered);
    let gutters = Arc::clone(&rendered);
    OverlayPalette::new(
        "Agent conversations",
        agent_conversations_key(),
        Msg::CloseAgentConversations,
        WIDTH,
    )
    .entries(entries)
    .preserve_groups(true)
    .item_key(|item| Arc::from(item.value.as_str()))
    .render_item(Arc::new(move |item, _highlight| {
        let row = items.iter().find(|row| row.id == item.value)?;
        Some(
            ListItem::from_spans(row.label.clone())
                .description_spans(row.description.clone())
                .primary_truncate_description_first(false)
                .primary_description_gap(2),
        )
    }))
    .item_gutter(Arc::new(move |item: &SearchItem<String>, _hl| {
        let row = gutters.iter().find(|row| row.id == item.value)?;
        let style = row.style;
        Some(if row.delivered {
            crate::view::vocabulary::picker_circle_spinner_gutter(style)
        } else {
            crate::view::vocabulary::picker_marker_gutter(row.glyph, style)
        })
    }))
    .actions(actions)
    .placeholder("Search conversations…")
    .initial_query(query.to_string())
    .selected(selected_row)
    .empty_text(empty_text)
    .fallback_interceptor(fallback)
    .on_query_change(
        ctx.link()
            .callback(|query: Arc<str>| Msg::AgentConversationsQueryChanged(query.to_string())),
    )
    .on_select(ctx.link().callback(|event: SearchEvent<String>| {
        Msg::AgentConversationsSelect(event.item.value.clone())
    }))
    .on_activate(ctx.link().callback(|event: SearchEvent<String>| {
        Msg::AgentConversationsOpenChain(event.item.value.clone())
    }))
    .render(ctx)
}

/// What one list row draws.
struct RenderedRow {
    id: String,
    root: String,
    search: String,
    label: Vec<Span>,
    description: Vec<Span>,
    glyph: &'static str,
    style: Style,
    waiting: bool,
    delivered: bool,
}

impl RenderedRow {
    fn new(
        state: &crate::state::State,
        request: &AgentRequestSummary,
        depth: usize,
        waiting: bool,
        now: u64,
        theme: &Theme,
    ) -> Self {
        use crate::view::vocabulary::{ASKING, fields};
        let from = agent_name(state, request.from.as_ref());
        let to = agent_name(state, Some(&request.to));
        let indent = "  ".repeat(depth.min(4));
        let lead = if depth > 0 { "└ " } else { "" };
        let label = vec![
            Span::new(format!("{indent}{lead}")).style(fg_only(&theme.muted)),
            Span::new(from.clone()),
            Span::new(format!(" {ASKING} ")).style(fg_only(&theme.muted)),
            Span::new(to.clone()),
        ];
        let age = if !request.state.is_resolved() {
            short_duration(now.saturating_sub(request.created_at_unix_ms))
        } else {
            short_duration(
                request
                    .updated_at_unix_ms
                    .saturating_sub(request.created_at_unix_ms),
            )
        };
        let (glyph, style) = match request.state {
            AgentRequestState::Queued => ("◌", fg_only(&theme.muted)),
            AgentRequestState::Active => ("●", fg_only(&theme.accent)),
            AgentRequestState::Answered => ("✓", Style::new().fg(theme.status.success)),
            AgentRequestState::Failed => ("✗", Style::new().fg(theme.status.error)),
            AgentRequestState::Cancelled => ("○", fg_only(&theme.muted)),
        };
        let reason = request
            .reason
            .clone()
            .filter(|_| {
                matches!(
                    request.state,
                    AgentRequestState::Queued | AgentRequestState::Failed
                )
            })
            .unwrap_or_default();
        let text = fields([state_word(request).to_string(), age, reason]);
        let description = vec![Span::new(text).style(style)];
        Self {
            id: request.id.clone(),
            root: request.root.clone(),
            search: format!("{from} {to} {}", request.id),
            label,
            description,
            glyph,
            style,
            waiting,
            delivered: request.state == AgentRequestState::Active,
        }
    }
}

/// One chain as a sequence diagram: a lifeline per agent, an arrow for each request asked, an
/// activation bar while its agent answers, and a dashed arrow back when it settles.
fn chain_overlay(ctx: &Context<AppRoot>, root: &str) -> Element {
    let theme = &ctx.state.theme;
    let all = requests(&ctx.state);
    let events = chain_events(all, root);
    let now = now_ms();
    let mut participants: Vec<String> = Vec::new();
    let mut add = |name: String| {
        if !participants.contains(&name) {
            participants.push(name);
        }
    };
    let names: Vec<(String, String)> = events
        .iter()
        .map(|event| {
            let request = match event {
                ChainEvent::Ask(request) | ChainEvent::Settle(request) => request,
            };
            (
                agent_name(&ctx.state, request.from.as_ref()),
                agent_name(&ctx.state, Some(&request.to)),
            )
        })
        .collect();
    for (from, to) in &names {
        add(from.clone());
        add(to.clone());
    }
    let mut diagram = SequenceDiagram::new()
        .minimal()
        .max_label_cells(Some(28))
        .lifeline_style(fg_only(&theme.muted))
        .participant_style(rozi_chrome(theme).bold())
        .message_label_style(fg_only(&theme.primary))
        .activation_style(fg_only(&theme.accent))
        .padding((1, 2, 1, 2));
    for name in &participants {
        diagram = diagram.participant(name.clone());
    }
    let muted = fg_only(&theme.muted);
    for (event, (from, to)) in events.iter().zip(names) {
        diagram = diagram.message(match event {
            ChainEvent::Ask(request) => {
                let label = match request.state {
                    AgentRequestState::Queued => format!(
                        "{} {}",
                        state_word(request),
                        short_duration(now.saturating_sub(request.created_at_unix_ms))
                    ),
                    // Promised a later answer: the open bar is the agent still owing it.
                    AgentRequestState::Active if request.reason.is_some() => {
                        "ask · later".to_string()
                    }
                    _ => "ask".to_string(),
                };
                let delivered = request.state != AgentRequestState::Queued
                    && !(request.state == AgentRequestState::Cancelled
                        && request.updated_at_unix_ms == request.created_at_unix_ms);
                let message = if request.state == AgentRequestState::Queued {
                    SequenceMessage::async_(from.as_str(), to.as_str(), label).label_style(muted)
                } else {
                    SequenceMessage::sync(from.as_str(), to.as_str(), label)
                };
                message.activate_target(delivered)
            }
            ChainEvent::Settle(request) => {
                let took = short_duration(
                    request
                        .updated_at_unix_ms
                        .saturating_sub(request.created_at_unix_ms),
                );
                let (word, style) = match request.state {
                    AgentRequestState::Answered => {
                        ("answer", Style::new().fg(theme.status.success))
                    }
                    AgentRequestState::Failed => ("failed", Style::new().fg(theme.status.error)),
                    _ => ("cancelled", fg_only(&theme.muted)),
                };
                let delivered = request.state != AgentRequestState::Cancelled
                    || request.updated_at_unix_ms != request.created_at_unix_ms;
                SequenceMessage::reply(to.as_str(), from.as_str(), format!("{word} · {took}"))
                    .label_style(style)
                    .line_style(style)
                    .deactivate_source(delivered)
            }
        });
    }
    let waiting = all
        .iter()
        .filter(|request| request.root == root && !request.state.is_resolved())
        .count();
    let selected = ctx
        .state
        .agent_conversations
        .as_ref()
        .and_then(|view| view.selected.clone());
    let actions = vec![
        OverlayAction::new("enter", "back", Msg::AgentConversationsBack, true),
        OverlayAction::new(
            "ctrl-g",
            "go to",
            selected
                .clone()
                .map(Msg::AgentConversationsGoTo)
                .unwrap_or(Msg::AgentConversationsBack),
            selected.is_some(),
        ),
    ];
    let mut interceptor = overlay_interceptor(ctx, &actions);
    let back = ctx
        .link()
        .key_handler(|key| key.is(KeyCode::Esc).then_some(Msg::AgentConversationsBack));
    let keys = KeyHandler::new(move |key| interceptor.handle(key) || back.handle(key));
    interceptor = keys;
    let max_height = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(CHAIN_CHROME_ROWS)
        .max(5);
    let body = ScrollView::new()
        .child(diagram)
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(theme))
        .height(Length::Auto)
        .max_height(Length::Px(max_height));
    let content = VStack::new()
        .height(Length::Auto)
        .child(
            KeyCapture::new()
                .tab_stop(false)
                .on_key(interceptor)
                .key(agent_conversations_chain_key()),
        )
        .child(body)
        .child(overlay_hints(ctx, &actions));
    let count = participants_count_label(participants.len(), waiting);
    wrap_palette(
        ctx,
        std::borrow::Cow::Owned(format!("Conversation · {root}")),
        Some(std::borrow::Cow::Owned(count)),
        "rozi-agent-conversation-chain",
        Msg::AgentConversationsBack,
        content,
        WIDTH,
        0,
    )
}

fn participants_count_label(agents: usize, waiting: usize) -> String {
    let agents = crate::view::vocabulary::count(agents, "agent");
    match waiting {
        0 => agents,
        waiting => format!("{agents}{SEPARATOR}{waiting} waiting"),
    }
}
