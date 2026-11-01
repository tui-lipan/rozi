use super::*;

const RELEASE_NOTES_WIDTH: u16 = 80;
/// Modal border and the two-row hint footer.
const CHROME_ROWS: u16 = 4;

/// What's new in this build: its release notes, rendered as Markdown.
pub(crate) fn release_notes_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(notes) = ctx.state.release_notes.as_ref() else {
        return Text::new("").into();
    };
    let actions = vec![
        OverlayAction::new("enter", "close", Msg::CloseReleaseNotes, true),
        OverlayAction::new(
            "ctrl-l",
            "open release page",
            Msg::OpenReleaseNotesPage,
            true,
        ),
    ];
    let max_height = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(CHROME_ROWS)
        .max(3);
    let document = DocumentView::new(notes.markdown)
        .markdown()
        .height(Length::Auto)
        .wrap(true)
        .line_numbers(false)
        .border(false)
        .padding((0, 1, 0, 1))
        .scrollbar(true)
        .scrollbar_config(modal_scrollbar_config(&ctx.state.theme))
        // The notes are the modal's only focusable widget, so they are its tab stop and the arrows
        // and Page keys scroll them.
        .focusable(true)
        .style(fg_only(&ctx.state.theme.primary))
        .focus_content_style(fg_only(&ctx.state.theme.primary))
        .on_key(overlay_interceptor(ctx, &actions))
        .key(release_notes_key())
        .max_height(Length::Px(max_height));
    let content = VStack::new()
        .height(Length::Auto)
        .child(document)
        .child(overlay_hints(ctx, &actions));
    let title = format!("What's new · rozi v{}", notes.version);

    let mut modal = action_palette_modal_with_width(ctx, &title, RELEASE_NOTES_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseReleaseNotes))
        .child(content);
    if ctx.state.release_notes_is_stacked() {
        modal = modal
            .backdrop_style(receding_backdrop(&ctx.state.theme))
            .reserve_height(crate::view::stacked_reserve(
                ctx,
                ACTION_PALETTE_MAX_HEIGHT_PERCENT,
                1,
            ));
    }
    modal.key("rozi-release-notes-modal")
}
