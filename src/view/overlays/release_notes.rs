use std::borrow::Cow;

use super::*;

const RELEASE_NOTES_WIDTH: u16 = 80;
/// Modal border and the two-row hint footer.
const CHROME_ROWS: u16 = 4;
/// The tab row and the blank row under it.
const TAB_ROWS: u16 = 2;

/// What's new: the release notes this build carries, one release per tab, newest first, rendered
/// as Markdown.
pub(crate) fn release_notes_overlay(ctx: &Context<AppRoot>) -> Element {
    let Some(whats_new) = ctx.state.release_notes.as_ref() else {
        return Text::new("").into();
    };
    let active = ctx
        .state
        .release_notes_tab
        .min(whats_new.releases.len() - 1);
    let notes = whats_new.release(active);
    let tabs = (whats_new.releases.len() > 1).then(|| {
        OverlayTabs::new(
            whats_new
                .releases
                .iter()
                .map(|notes| format!("v{}", notes.version))
                .collect(),
            active,
            Msg::ReleaseNotesTab,
        )
    });
    let actions = vec![
        OverlayAction::new("enter", "close", Msg::CloseReleaseNotes, true),
        OverlayAction::new(
            "ctrl-l",
            "open release page",
            Msg::OpenReleaseNotesPage,
            true,
        ),
    ];
    // Caller actions first, then the tab keys, as on every tabbed picker.
    let mut interceptors = vec![overlay_interceptor(ctx, &actions)];
    interceptors.extend(tabs.as_ref().map(|tabs| tabs.interceptor(ctx)));
    let interceptor =
        KeyHandler::new(move |key| interceptors.iter().any(|handler| handler.handle(key)));

    let chrome = CHROME_ROWS + if tabs.is_some() { TAB_ROWS } else { 0 };
    let max_height = (ctx.viewport().h * ACTION_PALETTE_MAX_HEIGHT_PERCENT / 100)
        .saturating_sub(chrome)
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
        .on_key(interceptor)
        .key(release_notes_key())
        .max_height(Length::Px(max_height));
    let mut content = VStack::new().height(Length::Auto);
    if let Some(tabs) = tabs.as_ref() {
        content = content.child(tabs.strip(ctx));
    }
    let content = content.child(document).child(overlay_hints(ctx, &actions));

    // With tabs, the active one names the release.
    let title = if tabs.is_some() {
        "What's new · rozi".to_string()
    } else {
        format!("What's new · rozi v{}", notes.version)
    };
    // The releases this machine had not run lead the list; say how many, and since when.
    let news = whats_new
        .last_ran
        .as_ref()
        .filter(|_| whats_new.new_count() > 0)
        .map(|last| {
            Cow::Owned(format!(
                "{} since v{last}",
                crate::view::vocabulary::count(whats_new.new_count(), "new release")
            ))
        });
    let depth = u16::from(ctx.state.release_notes_is_stacked());
    wrap_palette(
        ctx,
        Cow::Owned(title),
        news,
        "rozi-release-notes-modal",
        Msg::CloseReleaseNotes,
        content,
        RELEASE_NOTES_WIDTH,
        depth,
    )
}
