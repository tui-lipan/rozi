/// The chips of a two-answer dialog, in the order the row draws them.
///
/// The refusal comes first and the affirmative last, where a dialog's commit belongs. The
/// affirmative is also what the dialog opens on: it is the answer the user came for, and the other
/// one is a single arrow key — or `Esc` — away.
pub(crate) const DIALOG_REFUSE: usize = 0;
pub(crate) const DIALOG_AFFIRM: usize = 1;

/// One answer in a dialog's button row.
///
/// Focus *is* the selection here, so a chip carries the message that moves focus onto it as well
/// as the one that commits it: its neighbours send the first, and `Enter`, `Space`, or a click
/// send the second.
struct DialogButton<'a> {
    label: &'a str,
    answer: Msg,
    focus: Msg,
    /// Fill with the error accent rather than the ordinary focus highlight, for an answer that
    /// destroys something. The same signal an armed picker row and an armed prompt already use.
    destructive: bool,
}

impl<'a> DialogButton<'a> {
    fn new(label: &'a str, answer: Msg, focus: Msg) -> Self {
        Self {
            label,
            answer,
            focus,
            destructive: false,
        }
    }
}

/// The answer row shared by dialogs whose question has a fixed set of answers rather than a field.
///
/// Right-aligned, because the question above it is left-aligned: the eye finishes the sentence and
/// lands on the answers. The chip with focus wears the highlight a picker's selected row wears, so
/// "what `Enter` does" looks the same across the app whether it is a row or a button.
fn dialog_button_row(ctx: &Context<AppRoot>, close: &Msg, buttons: &[DialogButton<'_>]) -> Element {
    let theme = &ctx.state.theme;
    // No leading gap of its own: every row above it already ends with one, and the modal's own top
    // padding covers a dialog whose question fits in the title. It is the last row, so it carries
    // the trailing gap that keeps the chips off the modal's border.
    let mut row = HStack::new()
        .height(Length::Auto)
        .padding((0, 1, 1, 1))
        .justify(Justify::End);
    for (index, button) in buttons.iter().enumerate() {
        // Wrapping, so two chips swap on either arrow and a longer row still walks end to end.
        let previous = buttons[(index + buttons.len() - 1) % buttons.len()].focus.clone();
        let next = buttons[(index + 1) % buttons.len()].focus.clone();
        let close = close.clone();
        let answer = button.answer.clone();
        let key = crate::view::dialog_answer_key(index);
        // A filled chip already carries a background, so lifting the background is what reads as
        // hover on it; an unfilled one has only its text to lift.
        let hover = if ctx.has_focus_within_key(key.clone()) {
            Style::new().transform_bg(crate::view::hover_lift())
        } else {
            Style::new().transform_fg(crate::view::hover_lift())
        };
        row = row.child(
            Button::filled(button.label)
                .style(fg_only(&theme.muted))
                .focus_style(picker_selection_style(
                    theme,
                    button.destructive.then_some(theme.status.error),
                ))
                .hover_style(hover)
                .padding((0, 3))
                .on_click(ctx.link().callback(move |_| answer.clone()))
                // `Enter` and `Space` fall through to the button's own activation, which is what
                // fires `on_click`; only movement and the escape hatch are claimed here.
                .on_key(ctx.link().key_handler(move |key| {
                    match key.code {
                        KeyCode::Left | KeyCode::Char('h') => Some(previous.clone()),
                        KeyCode::Right | KeyCode::Char('l') => Some(next.clone()),
                        KeyCode::Esc => Some(close.clone()),
                        _ => None,
                    }
                }))
                .key(key),
        );
    }
    row.into()
}

/// What the dialog has to say for itself, on the line above the answers. The scrollable and
/// spinner forms belong to the prompts that opt into them; a dialog gets the plain line.
fn dialog_caption_row(theme: &Theme, caption: PromptCaption<'_>) -> Element {
    let accent = prompt_caption_accent(theme, caption);
    HStack::new()
        .height(Length::Auto)
        .padding((0, 1, 1, 1))
        .child(
            Text::new(caption.text())
                .overflow(Overflow::Wrap)
                .width(Length::Flex(1))
                .style(Style::new().fg(accent).italic()),
        )
        .into()
}

/// What one chosen-answer dialog differs by. The mirror of [`PromptChrome`] for a question with
/// no field: same modal, same body slots, an answer row where the input would be.
struct DialogChrome<'a> {
    title: &'a str,
    /// Wrapped text between the title and the answers, for a question too long to be a title.
    detail: Option<&'a str>,
    /// One string out of [`Self::detail`] repeated unbroken on its own line. See
    /// [`PromptChrome::highlight`].
    highlight: Option<&'a str>,
    /// An inline caption above the hints. See [`PromptCaption`] for what each kind costs the
    /// chrome.
    caption: Option<PromptCaption<'a>>,
    /// Fade whatever is already on screen behind this dialog. See [`PromptChrome::dim_behind`].
    dim_behind: bool,
}

/// Shared chrome for the dialogs answered by choosing rather than typing: the palette modal every
/// prompt wears, the same detail and caption rows, and a right-aligned answer row in place of the
/// field.
fn dialog_overlay(
    ctx: &Context<AppRoot>,
    chrome: DialogChrome<'_>,
    close: Msg,
    buttons: &[DialogButton<'_>],
) -> Element {
    let DialogChrome {
        title,
        detail,
        highlight,
        caption,
        dim_behind,
    } = chrome;
    let theme = &ctx.state.theme;
    let mut body = VStack::new().height(Length::Auto).padding((1, 0, 0, 0));
    if let Some(detail) = detail {
        body = body.child(prompt_detail_row(theme, detail));
    }
    if let Some(highlight) = highlight {
        body = body.child(prompt_highlight_row(theme, highlight));
    }
    // Above the answers, not below them: a rejected answer or a warning is part of the question
    // being asked again, and belongs on the reading side of the row that resolves it.
    if let Some(caption) = caption {
        body = body.child(dialog_caption_row(theme, caption));
    }
    // No hint bar. Every pill a dialog could carry names something the row already shows: the
    // highlight says what `Enter` does, the second chip says the other answer is reachable, and
    // `cancel esc` would put a second name on what the refusal chip is already offering.
    body = body.child(dialog_button_row(ctx, &close, buttons));

    let mut modal = action_palette_modal(ctx, title)
        .on_close(ctx.link().callback(move |_| close.clone()))
        .child(action_palette_frame(body));
    if dim_behind {
        modal = modal
            .backdrop_style(Style::new().tint_by(theme.surface.backdrop, BACKDROP_RECESSION));
    }
    modal.into()
}
