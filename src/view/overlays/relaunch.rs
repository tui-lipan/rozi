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
                "Restart"
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

/// The build that would run, then one row per session and per process this client runs itself,
/// then the release-wide note an update raised. `None` while there is no plan and no note.
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
    let mut rows = VStack::new().height(Length::Auto).padding((0, 1, 1, 1));
    if let Some(plan) = plan {
        rows = rows.child(build_row(theme, plan));
        let styles = SessionStatusStyles::from_theme(theme);
        for session in &plan.sessions {
            let target = session.facts.remote.as_ref();
            let presence =
                vocabulary::session_presence_for(&ctx.state, &session.facts.name, target);
            let host = target.map(crate::session::remote::RemoteTarget::display_label);
            let (status, reason) = vocabulary::relaunch_transition(&session.transition);
            rows = rows.child(item_row(
                theme,
                Span::new(presence.marker()).style(presence.style(styles)),
                &vocabulary::session_at(&session.facts.name, host.as_deref()),
                Span::new(status).style(transition_style(theme, &session.transition)),
            ));
            if let Some(reason) = reason {
                rows = rows.child(reason_row(theme, reason));
            }
        }
        for process in &plan.ending {
            rows = rows.child(item_row(
                theme,
                Span::new(vocabulary::CLIENT_PROCESS_MARKER).style(fg_only(&theme.muted)),
                process,
                Span::new(vocabulary::RELAUNCH_ENDS).style(Style::new().fg(theme.status.warning)),
            ));
        }
    }
    if let Some(note) = &dialog.note {
        rows = rows.child(
            HStack::new()
                .height(Length::Auto)
                .padding((u16::from(plan.is_some()), 0, 0, 0))
                .child(
                    Text::from_spans([
                        Span::new("extensions  ").style(fg_only(&theme.muted)),
                        Span::new(note.as_str()).style(fg_only(&theme.primary)),
                    ])
                    .overflow(Overflow::Wrap)
                    .width(Length::Flex(1)),
                ),
        );
    }
    Some(rows.into())
}

/// `build  v1.2.3 · /path/to/rozi`: the release is the subject of the question, so it wears the
/// accent; the path is only where it comes from, muted, and cut short first when it is long.
fn build_row(theme: &Theme, plan: &RelaunchPlan) -> Element {
    let muted = fg_only(&theme.muted);
    HStack::new()
        .height(Length::Auto)
        .padding((0, 0, 1, 0))
        .child(Text::new("build  ").style(muted))
        .child(
            Text::new(format!("v{}", plan.candidate.release)).style(fg_only(&theme.accent).bold()),
        )
        .child(Text::new(vocabulary::SEPARATOR).style(muted))
        .child(
            Text::new(plan.candidate.path.display().to_string())
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .style(muted),
        )
        .into()
}

/// One item and what becomes of it, laid out as a picker row: the marker, the name in the body
/// colour, and the status right-aligned.
fn item_row(theme: &Theme, marker: Span, name: &str, status: Span) -> Element {
    HStack::new()
        .height(Length::Px(1))
        .child(Text::from_spans([marker, Span::new(" ")]))
        .child(
            Text::new(name)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .style(fg_only(&theme.primary)),
        )
        .child(Text::from_spans([Span::new("  "), status]))
        .into()
}

/// Why a session is blocked, under its row and level with its name.
fn reason_row(theme: &Theme, reason: &str) -> Element {
    HStack::new()
        .height(Length::Auto)
        .padding((0, 0, 0, 2))
        .child(
            Text::new(reason)
                .overflow(Overflow::Wrap)
                .width(Length::Flex(1))
                .style(Style::new().fg(theme.status.error).italic()),
        )
        .into()
}

/// Calm when the session comes through untouched, louder the more it costs: a reconnect is news,
/// an upgrade ends programs, and a blocked session stops the restart.
fn transition_style(theme: &Theme, transition: &Transition) -> Style {
    match transition {
        Transition::Carry { .. } => fg_only(&theme.muted),
        Transition::Reconnect => Style::new().fg(theme.status.info),
        Transition::Upgrade => Style::new().fg(theme.status.warning),
        Transition::Blocked { .. } => Style::new().fg(theme.status.error).bold(),
    }
}
