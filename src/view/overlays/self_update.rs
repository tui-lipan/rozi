//! The **Update rozi** card: the release it moves to, each step as it happens, and what stopped it.

use super::confirm::{DialogButton, DialogChrome, dialog_overlay};
use super::*;
use crate::state::{OUTPUT_LINES, SelfUpdateRun, UpdateChannel, UpdatePhase, UpdateStep};
use crate::view::vocabulary;

/// The card's identity across its steps and into the Restart rozi dialog it becomes, so one card
/// changes rather than one closing as the next opens.
pub(super) const CARD_KEY: &str = "rozi-update-card";

const VERSION: &str = "version";
const VIA: &str = "via";

pub(crate) fn self_update_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(run) = ctx.state.self_update.run.as_ref().filter(|run| !run.hidden) else {
        return Spacer::new().into();
    };
    let title = match &run.phase {
        UpdatePhase::Installed => format!("rozi v{} installed", run.release()),
        UpdatePhase::Failed { .. } => "rozi update failed".to_string(),
        _ => "Updating rozi".to_string(),
    };
    let buttons = match &run.phase {
        UpdatePhase::Failed { .. } => vec![
            DialogButton::new(
                "Close",
                Msg::SelfUpdateHide,
                Msg::SelfUpdateFocusAnswer(DIALOG_REFUSE),
            ),
            DialogButton::new(
                "Try again",
                Msg::SelfUpdateRetry,
                Msg::SelfUpdateFocusAnswer(DIALOG_AFFIRM),
            ),
        ],
        // Nothing to answer: the card is about to become the restart dialog.
        UpdatePhase::Installed => Vec::new(),
        // Says that the update goes on: "Hide" alone reads as if it might stop it.
        _ => vec![DialogButton::new(
            "Run in background",
            Msg::SelfUpdateHide,
            Msg::SelfUpdateFocusAnswer(DIALOG_REFUSE),
        )],
    };
    // A settled card reports its outcome in its frame; with no answer to focus, it would otherwise
    // fall back to the resting border while it waits to become the restart dialog.
    let theme = &ctx.state.theme;
    let tone = match &run.phase {
        UpdatePhase::Installed => Some(theme.status.success),
        UpdatePhase::Failed { .. } => Some(theme.status.error),
        _ => None,
    };
    dialog_overlay(
        ctx,
        DialogChrome {
            title: &title,
            detail: None,
            facts: &[],
            rows: Some(rows(&ctx.state.theme, run)),
            highlight: None,
            caption: None,
            dim_behind: true,
            parent_reserve_percent: None,
            layers: 1,
            key: Some(CARD_KEY),
            tone,
        },
        Msg::SelfUpdateHide,
        &buttons,
    )
}

/// The release step first, then the steps of getting there.
fn rows(theme: &Theme, run: &SelfUpdateRun) -> Element {
    let muted = fg_only(&theme.muted);
    let label_width = VERSION.len().max(VIA.len());
    let label = |text: &str| {
        Text::new(format!("{text:<label_width$}  "))
            .overflow(Overflow::Clip)
            .style(muted)
    };
    let mut rows = VStack::new().height(Length::Auto).padding((0, 1, 1, 1));
    rows = rows.child(
        HStack::new()
            .height(Length::Px(1))
            .child(label(VERSION))
            .child(Text::from_spans([
                Span::new(format!("v{} → ", run.running)).style(muted),
                Span::new(format!("v{}", run.release())).style(fg_only(&theme.accent).bold()),
            ])),
    );
    let gap = HStack::new().height(Length::Px(1));
    match run.channel {
        UpdateChannel::Managed => {
            rows = rows.child(gap);
            rows = rows.child(download_row(theme, run));
            rows = rows.child(install_row(theme, run));
        }
        UpdateChannel::Command { command, .. } => {
            rows = rows.child(
                HStack::new()
                    .height(Length::Px(1))
                    .child(label(VIA))
                    .child(Text::new(command).style(fg_only(&theme.primary))),
            );
            rows = rows.child(gap);
            rows = rows.child(step_row(
                theme,
                marker(theme, &run.phase, UpdateStep::Command),
                "Package manager",
                status(theme, &run.phase, UpdateStep::Command),
            ));
            rows = output_rows(theme, rows, run);
        }
    }
    if let UpdatePhase::Failed { reason, .. } = &run.phase {
        rows = rows.child(
            HStack::new()
                .height(Length::Auto)
                .padding((1, 0, 0, 0))
                .child(
                    Text::new(reason.as_str())
                        .overflow(Overflow::Wrap)
                        .width(Length::Flex(1))
                        .style(Style::new().fg(theme.status.error).italic()),
                ),
        );
    }
    rows.into()
}

/// The download: a bar in the mark's colours while it arrives, its size once it has.
fn download_row(theme: &Theme, run: &SelfUpdateRun) -> Element {
    let row = HStack::new()
        .height(Length::Px(1))
        .child(marker(theme, &run.phase, UpdateStep::Download))
        .child(Text::new(format!("{:<9}", "Download")).style(fg_only(&theme.primary)));
    match run.phase {
        UpdatePhase::Downloading {
            downloaded,
            total: Some(total),
        } if total > 0 => {
            let fraction = (downloaded as f64 / total as f64).clamp(0.0, 1.0);
            row.child(progress_bar(theme, fraction))
                .child(
                    Text::new(format!(
                        "  {}",
                        crate::platform::progress::bytes(downloaded)
                    ))
                    .style(fg_only(&theme.muted)),
                )
                .into()
        }
        UpdatePhase::Downloading { downloaded, .. } => {
            let readout = if downloaded == 0 {
                "Connecting…".to_string()
            } else {
                crate::platform::progress::bytes(downloaded)
            };
            row.child(Spacer::new().width(Length::Flex(1)))
                .child(Text::new(readout).style(fg_only(&theme.muted)))
                .into()
        }
        _ => {
            let status = match (&run.phase, run.downloaded) {
                (UpdatePhase::Failed { step, .. }, _) if *step == UpdateStep::Download => {
                    status(theme, &run.phase, UpdateStep::Download)
                }
                (_, Some(size)) => {
                    Span::new(crate::platform::progress::bytes(size)).style(fg_only(&theme.muted))
                }
                _ => Span::new(""),
            };
            row.child(Spacer::new().width(Length::Flex(1)))
                .child(Text::from_spans([status]))
                .into()
        }
    }
}

fn install_row(theme: &Theme, run: &SelfUpdateRun) -> Element {
    step_row(
        theme,
        marker(theme, &run.phase, UpdateStep::Install),
        "Install",
        status(theme, &run.phase, UpdateStep::Install),
    )
}

/// A step: its marker, its name, and its status right-aligned, as a picker row reads.
fn step_row(theme: &Theme, marker: Element, name: &str, status: Span) -> Element {
    HStack::new()
        .height(Length::Px(1))
        .child(marker)
        .child(
            Text::new(name)
                .overflow(Overflow::Ellipsis)
                .width(Length::Flex(1))
                .style(fg_only(&theme.primary)),
        )
        .child(Text::from_spans([status]))
        .into()
}

/// The fill runs through the mark's own gradient, the same in every theme, over a quiet track.
fn progress_bar(theme: &Theme, fraction: f64) -> Element {
    let (top, bottom) = (super::welcome::LOGO_TOP, super::welcome::LOGO_BOTTOM);
    ProgressBar::new(fraction)
        .progress_style(ProgressStyle::Line)
        .filled_gradient(ColorGradient::new(
            Color::Rgb(top.0, top.1, top.2),
            Color::Rgb(bottom.0, bottom.1, bottom.2),
        ))
        .empty_style(fg_only(&theme.border))
        .show_percentage(true)
        .label_style(fg_only(&theme.muted))
        .width(Length::Flex(1))
        .into()
}

/// Where a step stands: waiting, working, done, or where the update stopped.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Standing {
    Waiting,
    Working,
    Done,
    Stopped,
}

fn standing(phase: &UpdatePhase, step: UpdateStep) -> Standing {
    let order = |step: UpdateStep| match step {
        UpdateStep::Download => 0,
        UpdateStep::Install | UpdateStep::Command => 1,
    };
    match phase {
        UpdatePhase::Installed => Standing::Done,
        UpdatePhase::Failed { step: stopped, .. } if *stopped == step => Standing::Stopped,
        UpdatePhase::Failed { step: stopped, .. } if order(*stopped) > order(step) => {
            Standing::Done
        }
        UpdatePhase::Failed { .. } => Standing::Waiting,
        phase if phase.step() == step => Standing::Working,
        phase if order(phase.step()) > order(step) => Standing::Done,
        _ => Standing::Waiting,
    }
}

/// A spinner while the step works; the session family's settled marks otherwise.
fn marker(theme: &Theme, phase: &UpdatePhase, step: UpdateStep) -> Element {
    let glyph = |glyph: &str, style: Style| -> Element {
        Text::new(format!("{glyph} ")).style(style).into()
    };
    match standing(phase, step) {
        Standing::Working => HStack::new()
            .width(Length::Px(2))
            .height(Length::Px(1))
            .child(vocabulary::picker_circle_spinner(fg_only(&theme.accent)))
            .into(),
        Standing::Done => glyph("●", Style::new().fg(theme.status.success)),
        Standing::Stopped => glyph("×", Style::new().fg(theme.status.error).bold()),
        Standing::Waiting => glyph("○", fg_only(&theme.muted)),
    }
}

/// The step's status word: work in progress ends with `…`, settled states do not.
fn status(theme: &Theme, phase: &UpdatePhase, step: UpdateStep) -> Span {
    let (word, style) = match (standing(phase, step), step) {
        (Standing::Working, UpdateStep::Download) => ("Downloading…", fg_only(&theme.muted)),
        (Standing::Working, UpdateStep::Install) => ("Installing…", fg_only(&theme.muted)),
        (Standing::Working, UpdateStep::Command) => ("Running…", fg_only(&theme.muted)),
        (Standing::Done, _) => ("Done", Style::new().fg(theme.status.success)),
        (Standing::Stopped, _) => ("Failed", Style::new().fg(theme.status.error).bold()),
        (Standing::Waiting, _) => ("", fg_only(&theme.muted)),
    };
    Span::new(word).style(style)
}

/// What the package manager printed last, under its row and level with its name. Once there is any,
/// the card keeps room for [`OUTPUT_LINES`] so it does not grow a row at a time.
fn output_rows(theme: &Theme, mut rows: VStack, run: &SelfUpdateRun) -> VStack {
    if run.output.is_empty() {
        return rows;
    }
    for index in 0..OUTPUT_LINES {
        let line = run.output.get(index).map(String::as_str).unwrap_or("");
        rows = rows.child(
            HStack::new()
                .height(Length::Px(1))
                .padding((0, 0, 0, 2))
                .child(
                    Text::new(line)
                        .overflow(Overflow::Ellipsis)
                        .width(Length::Flex(1))
                        .style(fg_only(&theme.muted)),
                ),
        );
    }
    rows
}
