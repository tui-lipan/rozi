//! The Restart rozi dialog: which build would run, and what happens to each session.

use super::confirm::{DialogButton, DialogChrome, dialog_overlay};
use super::prompts::PromptCaption;
use super::*;
use crate::state::{RelaunchDialog, RelaunchPlan, RelaunchStage};

pub(crate) fn relaunch_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(dialog) = ctx.state.relaunch.dialog.as_ref() else {
        return Spacer::new().into();
    };
    let title = match &dialog.expected {
        Some(version) => format!("rozi v{version} installed"),
        None => "Restart rozi".to_string(),
    };
    let rows = facts(dialog);
    let facts: Vec<(&str, &str)> = rows
        .iter()
        .map(|(label, value)| (label.as_str(), value.as_str()))
        .collect();
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
                    ),
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
            facts: &facts,
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

/// The build first, then one row per session, then what ends with this client.
fn facts(dialog: &RelaunchDialog) -> Vec<(String, String)> {
    let plan: Option<&RelaunchPlan> = match &dialog.stage {
        RelaunchStage::Ready(plan) => Some(plan),
        RelaunchStage::Preparing(transaction) | RelaunchStage::Committing(transaction) => {
            Some(&transaction.plan)
        }
        RelaunchStage::Checking | RelaunchStage::Unavailable(_) | RelaunchStage::Aborting(_) => {
            None
        }
    };
    let mut rows = Vec::new();
    if let Some(plan) = plan {
        rows.push((
            "rozi".to_string(),
            crate::view::vocabulary::fields([
                format!("v{}", plan.candidate.release),
                plan.candidate.path.display().to_string(),
            ]),
        ));
        for session in &plan.sessions {
            let host = session
                .facts
                .remote
                .as_ref()
                .map(crate::session::remote::RemoteTarget::display_label);
            rows.push((
                crate::view::vocabulary::session_at(&session.facts.name, host.as_deref()),
                crate::view::vocabulary::relaunch_transition(&session.transition),
            ));
        }
        for process in &plan.ending {
            rows.push((
                process.clone(),
                crate::view::vocabulary::RELAUNCH_ENDS.to_string(),
            ));
        }
    }
    if let Some(note) = &dialog.note {
        rows.push(("extensions".to_string(), note.clone()));
    }
    rows
}
