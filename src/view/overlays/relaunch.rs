//! The Restart rozi dialog: which build would run, and what happens to each session.

use super::confirm::{DialogButton, DialogChrome, dialog_overlay};
use super::prompts::PromptCaption;
use super::*;
use crate::relaunch::plan::Transition;
use crate::state::{RelaunchDialog, RelaunchPlan, RelaunchStage};
use crate::view::vocabulary::{self, SessionStatusStyles};

pub(crate) fn relaunch_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(dialog) = ctx.state.relaunch.dialog.as_ref() else {
        return Spacer::new().into();
    };
    let title = match &dialog.expected {
        Some(version) => format!("rozi v{version} installed"),
        None => "Restart rozi".to_string(),
    };
    let rows = plan_rows(ctx, dialog);
    let (caption, buttons) = match &dialog.stage {
        RelaunchStage::Checking => (
            Some(PromptCaption::Note("Checking the installed rozi…")),
            vec![cancel("Cancel")],
        ),
        RelaunchStage::Preparing(_) | RelaunchStage::Committing(_) => (
            Some(PromptCaption::Note("Handing over…")),
            vec![cancel("Cancel")],
        ),
        RelaunchStage::Aborting(_) => (
            Some(PromptCaption::Note("Taking back what was reserved…")),
            vec![cancel("Close")],
        ),
        RelaunchStage::Unavailable(_) => (None, vec![cancel("Close")]),
        RelaunchStage::Ready(plan) if plan.blocked() => (
            Some(PromptCaption::Note(
                "Deal with the blocked sessions first, then restart",
            )),
            vec![cancel("Close")],
        ),
        RelaunchStage::Ready(plan) => {
            // A first press that armed says what a second one does; otherwise, why the last
            // attempt did not go ahead, if it did not.
            let caption = if plan.armed {
                Some(PromptCaption::Armed(
                    "Programs listed above end · again to restart",
                ))
            } else {
                plan.notice.as_deref().map(PromptCaption::Note)
            };
            let affirm = if plan.destructive() {
                "Restart anyway"
            } else {
                "Restart now"
            };
            (
                caption,
                vec![
                    cancel("Later"),
                    DialogButton::new(
                        affirm,
                        Msg::RelaunchConfirm,
                        Msg::RelaunchFocusAnswer(DIALOG_AFFIRM),
                    )
                    .destructive(plan.destructive()),
                ],
            )
        }
    };
    let detail = match &dialog.stage {
        RelaunchStage::Unavailable(reason) => Some(reason.as_str()),
        _ => None,
    };
    dialog_overlay(
        ctx,
        DialogChrome {
            title: &title,
            detail,
            facts: &[],
            rows,
            highlight: None,
            caption,
            dim_behind: true,
            parent_reserve_percent: None,
            layers: 1,
            // Opened by a finished update, this is the update's card going on to its next step.
            key: dialog
                .expected
                .is_some()
                .then_some(super::self_update::CARD_KEY),
            tone: None,
        },
        Msg::RelaunchCancel,
        &buttons,
    )
}

fn cancel(label: &str) -> DialogButton<'_> {
    DialogButton::new(
        label,
        Msg::RelaunchCancel,
        Msg::RelaunchFocusAnswer(DIALOG_REFUSE),
    )
}

/// Something a restart touches: a session, or a process this client runs itself.
struct Item {
    marker: Span,
    name: String,
    /// Why it blocks the restart, for a blocked session.
    reason: Option<String>,
}

/// The items sharing one outcome, under that outcome's word.
struct Outcome {
    label: &'static str,
    style: Style,
    items: Vec<Item>,
    /// What the outcome costs, after the names.
    cost: Option<&'static str>,
}

/// The release this client moves from and to, then what the restart does, one row per outcome
/// with the costliest first, so a cost is never below the fold of a list of calm rows; then the
/// release-wide note an update raised. A restart that costs nothing reads as one line: everything
/// keeps running.
fn plan_rows(ctx: &Context<AppRoot>, dialog: &RelaunchDialog) -> Option<Element> {
    let plan: Option<&RelaunchPlan> = match &dialog.stage {
        RelaunchStage::Ready(plan) => Some(plan),
        RelaunchStage::Preparing(transaction) | RelaunchStage::Committing(transaction) => {
            Some(&transaction.plan)
        }
        RelaunchStage::Checking | RelaunchStage::Unavailable(_) | RelaunchStage::Aborting(_) => {
            None
        }
    };
    if plan.is_none() && dialog.note.is_none() {
        return None;
    }
    let theme = &ctx.state.theme;
    let outcomes = plan.map(|plan| outcomes(ctx, plan)).unwrap_or_default();
    let label_width = outcomes
        .iter()
        .map(|outcome| outcome.label)
        .chain(plan.map(|_| VERSION))
        .chain(dialog.note.as_ref().map(|_| EXTENSIONS))
        .map(|label| label.chars().count())
        .max()
        .unwrap_or_default();
    let label = |text: &str, style: Style| {
        Text::new(format!("{text:<label_width$}  "))
            .overflow(Overflow::Clip)
            .style(style)
    };
    let mut rows = VStack::new().height(Length::Auto).padding((0, 1, 1, 1));
    if let Some(plan) = plan {
        let muted = fg_only(&theme.muted);
        rows = rows.child(
            HStack::new()
                .height(Length::Auto)
                .padding((0, 0, u16::from(!outcomes.is_empty()), 0))
                .child(label(VERSION, muted))
                .child(Text::from_spans(version_spans(
                    theme,
                    crate::release_app::ROZI.version,
                    &plan.candidate.release.to_string(),
                ))),
        );
    }
    for outcome in &outcomes {
        rows = outcome_rows(theme, rows, outcome, &label, label_width);
    }
    if let Some(note) = &dialog.note {
        rows = rows.child(
            HStack::new()
                .height(Length::Auto)
                .child(label(EXTENSIONS, fg_only(&theme.muted)))
                .child(
                    Text::new(note.as_str())
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(fg_only(&theme.primary)),
                ),
        );
    }
    Some(rows.into())
}

const VERSION: &str = "version";

/// `v1.2.2 → v1.2.3`: where this client is, muted, and where it goes, in the accent. A rebuild of
/// the same release has nowhere to go, and shows the one release.
fn version_spans(theme: &Theme, running: &str, next: &str) -> Vec<Span> {
    let to = Span::new(format!("v{next}")).style(fg_only(&theme.accent).bold());
    if running == next {
        return vec![to];
    }
    vec![
        Span::new(format!("v{running} → ")).style(fg_only(&theme.muted)),
        to,
    ]
}
const EXTENSIONS: &str = "extensions";

/// The plan's sessions and client processes, grouped by what becomes of them, costliest first.
fn outcomes(ctx: &Context<AppRoot>, plan: &RelaunchPlan) -> Vec<Outcome> {
    let theme = &ctx.state.theme;
    let styles = SessionStatusStyles::from_theme(theme);
    let mut ranked: Vec<(u8, Outcome)> = Vec::new();
    let mut add = |rank: u8, label: &'static str, style: Style, cost, item: Item| match ranked
        .iter_mut()
        .find(|(other, _)| *other == rank)
    {
        Some((_, outcome)) => outcome.items.push(item),
        None => ranked.push((
            rank,
            Outcome {
                label,
                style,
                items: vec![item],
                cost,
            },
        )),
    };
    for session in &plan.sessions {
        let target = session.facts.remote.as_ref();
        let presence = vocabulary::session_presence_for(&ctx.state, &session.facts.name, target);
        let host = target.map(crate::session::remote::RemoteTarget::display_label);
        let (rank, style, cost, reason) = match &session.transition {
            Transition::Blocked { reason } => (
                0,
                Style::new().fg(theme.status.error).bold(),
                None,
                Some(reason.clone()),
            ),
            Transition::Upgrade => (
                1,
                Style::new().fg(theme.status.warning),
                Some(vocabulary::RELAUNCH_PROGRAMS_END),
                None,
            ),
            Transition::Reconnect => (3, Style::new().fg(theme.status.info), None, None),
            Transition::Carry { .. } => (4, Style::new().fg(theme.status.success), None, None),
        };
        add(
            rank,
            vocabulary::relaunch_transition(&session.transition),
            style,
            cost,
            Item {
                marker: Span::new(presence.marker()).style(presence.style(styles)),
                name: vocabulary::session_at(&session.facts.name, host.as_deref()),
                reason,
            },
        );
    }
    for process in &plan.ending {
        add(
            2,
            vocabulary::RELAUNCH_ENDS,
            Style::new().fg(theme.status.warning),
            None,
            Item {
                marker: Span::new(vocabulary::CLIENT_PROCESS_MARKER).style(fg_only(&theme.muted)),
                name: process.clone(),
                reason: None,
            },
        );
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    ranked.into_iter().map(|(_, outcome)| outcome).collect()
}

/// One outcome: its word, then its items side by side. A blocked session takes a row of its own,
/// with its reason under it: each one blocks for a reason of its own.
fn outcome_rows(
    theme: &Theme,
    mut rows: VStack,
    outcome: &Outcome,
    label: &impl Fn(&str, Style) -> Text,
    label_width: usize,
) -> VStack {
    let primary = fg_only(&theme.primary);
    let item_spans = |item: &Item| {
        [
            item.marker.clone(),
            Span::new(" "),
            Span::new(item.name.clone()).style(primary),
        ]
    };
    if outcome.items.iter().any(|item| item.reason.is_some()) {
        for (index, item) in outcome.items.iter().enumerate() {
            let word = if index == 0 { outcome.label } else { "" };
            rows = rows.child(
                HStack::new()
                    .height(Length::Auto)
                    .child(label(word, outcome.style))
                    .child(
                        Text::from_spans(item_spans(item))
                            .overflow(Overflow::Ellipsis)
                            .width(Length::Flex(1)),
                    ),
            );
            if let Some(reason) = &item.reason {
                // Level with the name, past the marker.
                let indent = u16::try_from(label_width + 4).unwrap_or(u16::MAX);
                rows = rows.child(
                    HStack::new()
                        .height(Length::Auto)
                        .padding((0, 0, 0, indent))
                        .child(
                            Text::new(reason.as_str())
                                .overflow(Overflow::Wrap)
                                .width(Length::Flex(1))
                                .style(Style::new().fg(theme.status.error).italic()),
                        ),
                );
            }
        }
        return rows;
    }
    let mut spans = Vec::new();
    for (index, item) in outcome.items.iter().enumerate() {
        if index > 0 {
            spans.push(Span::new("  "));
        }
        spans.extend(item_spans(item));
    }
    if let Some(cost) = outcome.cost {
        spans.push(Span::new(vocabulary::SEPARATOR).style(fg_only(&theme.muted)));
        spans.push(Span::new(cost).style(outcome.style));
    }
    rows.child(
        HStack::new()
            .height(Length::Auto)
            .child(label(outcome.label, outcome.style))
            .child(
                Text::from_spans(spans)
                    .overflow(Overflow::Wrap)
                    .width(Length::Flex(1)),
            ),
    )
}
