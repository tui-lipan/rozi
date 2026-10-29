use super::*;

const RELEASE_NOTES_WIDTH: u16 = 80;
/// Columns left for text inside the modal border and the document's side padding.
const NOTES_TEXT_WIDTH: u16 = RELEASE_NOTES_WIDTH - 4;

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
    let document = DocumentView::new(notes.markdown)
        .markdown()
        .height(notes_height(ctx, notes.markdown))
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
        .key(release_notes_key());
    let content = VStack::new()
        .child(document)
        .child(overlay_hints(ctx, &actions));
    let title = format!("What's new · rozi v{}", notes.version);

    action_palette_modal_with_width(ctx, &title, RELEASE_NOTES_WIDTH)
        .on_close(ctx.link().callback(|_| Msg::CloseReleaseNotes))
        .child(content)
        .key("rozi-release-notes-modal")
}

/// Height for the notes: content-sized while they fit, capped so they scroll once they do not.
///
/// The modal caps itself at a share of the viewport, and an auto-height document squeezed by that
/// cap clips its tail under the hint row instead of scrolling. The estimate counts the Markdown
/// source, which is never shorter than what it renders to: link targets and markup disappear.
fn notes_height(ctx: &Context<AppRoot>, markdown: &str) -> Length {
    // Modal border, hint row, and one row of slack.
    const CHROME_ROWS: u16 = 4;

    let cap = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(CHROME_ROWS)
        .max(3);
    let budget = usize::from(NOTES_TEXT_WIDTH);
    let rows: usize = markdown
        .lines()
        .map(|line| {
            let width = unicode_width::UnicodeWidthStr::width(line);
            // Word wrapping breaks before the edge, so count each row at three quarters of it.
            width.div_ceil(budget * 3 / 4).max(1)
        })
        .sum();
    if rows > usize::from(cap) {
        Length::Px(cap)
    } else {
        Length::Auto
    }
}
