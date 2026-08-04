use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::ops::focus::request_current_pane_focus;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{CopyModeState, Mode};

/// Enter copy mode on the focused pane: seed tui-lipan's navigator at the live cursor position
/// with no selection, and park scrollback at its current offset. Closes any open overlay first.
pub(crate) fn enter(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.current().focused_pane else {
        return Update::full();
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
        return Update::full();
    };
    let (cursor_row, cursor_col) = pane.terminal.cursor_position();
    let offset = pane.terminal.scrollback_offset();
    clear_copy_feedback(ctx);
    ctx.state.copy_mode = Some(CopyModeState {
        target,
        navigation: TerminalCopyMode::new(cursor_row, cursor_col, offset),
        search_matches: Vec::new(),
        search_current: 0,
        search_truncated: false,
    });
    ctx.state.mode = Mode::Copy;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    Update::full()
}

/// Leave copy mode. When `copy` is set the current selection (if any) is sent to the system
/// clipboard and flashed by the framework after the controlled selection is cleared.
pub(crate) fn exit(ctx: &mut Context<HyprmuxApp>, copy: bool) -> Update {
    let Some(state) = ctx.state.copy_mode.take() else {
        ctx.state.mode = Mode::Normal;
        ctx.state.commands_dirty = true;
        return Update::full();
    };

    let mut copied_selection = None;
    if copy {
        let prepared = find_pane_mut(&mut ctx.state, state.target).and_then(|pane| {
            let total = pane.terminal.total_scrollback_rows();
            let selection = state.navigation.selection(total)?;
            let text =
                pane.terminal
                    .selection_display_text(&selection, SelectionEnd::Inclusive, true);
            if text.is_empty() {
                return None;
            }
            let offset = pane.terminal.scrollback_offset();
            let viewport_rows = usize::from(pane.terminal.rows);
            Some((text, to_viewport(&selection, offset, total, viewport_rows)))
        });
        if let Some((text, projected)) = prepared {
            match ctx.clipboard().copy(&text) {
                Ok(()) => copied_selection = projected,
                Err(err) => {
                    crate::pty_events::notify_error(ctx, "Copy failed", err.to_string());
                }
            }
        }
    }

    if copied_selection.is_none()
        && let Some(pane) = find_pane_mut(&mut ctx.state, state.target)
    {
        pane.terminal.set_scrollback(0);
    }
    let feedback =
        copied_selection.map(|selection| flash_copy_feedback(ctx, state.target, selection));
    ctx.state.mode = Mode::Normal;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::with_command(feedback)
}

pub(crate) fn clear_copy_feedback(ctx: &mut Context<HyprmuxApp>) {
    ctx.state.copy_feedback_epoch = ctx.state.copy_feedback_epoch.wrapping_add(1);
    if let Some((attachment, target)) = ctx.state.copy_feedback_target.take()
        && let Some(pane) = ctx
            .state
            .attachment_for_epoch_mut(attachment)
            .and_then(|attachment| attachment.find_pane_mut(target))
    {
        pane.terminal.set_scrollback(0);
    }
}

pub(crate) fn flash_copy_feedback(
    ctx: &mut Context<HyprmuxApp>,
    target: crate::state::PaneId,
    selection: tui_lipan::utils::GridSelection,
) -> Command {
    clear_copy_feedback(ctx);
    if ctx.has_focus_within_key(crate::view::pane_terminal_key(target))
        && let Some(node_id) = ctx.focused_node_id()
    {
        ctx.flash_copy_feedback_range(node_id, selection);
    }
    let epoch = ctx.state.copy_feedback_epoch;
    let attachment = ctx.state.runtime_epoch;
    ctx.state.copy_feedback_target = Some((attachment, target));
    let duration = crate::app::clipboard_copy_feedback_duration(&ctx.state.config);
    Command::after(duration, move |link: CommandLink<crate::Msg>| {
        link.send(crate::Msg::CopyFeedbackExpired(attachment, target, epoch));
    })
}

pub(crate) fn expire_copy_feedback(
    ctx: &mut Context<HyprmuxApp>,
    attachment: u64,
    target: crate::state::PaneId,
    epoch: u64,
) -> Update {
    if ctx.state.copy_feedback_epoch != epoch
        || ctx.state.copy_feedback_target != Some((attachment, target))
    {
        return Update::none();
    }
    ctx.state.copy_feedback_target = None;
    if let Some(pane) = ctx
        .state
        .attachment_for_epoch_mut(attachment)
        .and_then(|attachment| attachment.find_pane_mut(target))
    {
        pane.terminal.set_scrollback(0);
    }
    Update::full()
}

/// Route a key while in copy mode. Returns `(handled, update)`; every key is consumed so
/// nothing leaks to the PTY, mirroring resize mode.
pub(crate) fn handle_copy_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Char('/')) {
        return (true, crate::ops::search::open_search_from_copy_mode(ctx));
    }
    // Match on `key.code` (not `key.is`) so Shift+N still works under Kitty keyboard protocol.
    if matches!(key.code, KeyCode::Char('n')) {
        return (true, cycle_copy_search(ctx, false));
    }
    if matches!(key.code, KeyCode::Char('N')) {
        return (true, cycle_copy_search(ctx, true));
    }
    if key.is(KeyCode::Char('o')) {
        return (true, crate::ops::last_output::copy_last_output(ctx));
    }

    let Some(target) = ctx.state.copy_mode.as_ref().map(|copy| copy.target) else {
        return (true, Update::none());
    };
    let cursor_row = ctx
        .state
        .copy_mode
        .as_ref()
        .map(|copy| copy.navigation.cursor().0)
        .unwrap_or(0);
    let (rows, cols, total_scrollback_rows, row_text, prompt_lines) = {
        let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
            return (true, Update::none());
        };
        let snapshot = pane.terminal.snapshot();
        let row_text = snapshot
            .text
            .lines()
            .nth(cursor_row)
            .unwrap_or("")
            .trim_end()
            .to_string();
        let mut prompt_lines: Vec<usize> = pane
            .terminal
            .semantic_marks()
            .into_iter()
            .filter(|mark| mark.kind == SemanticMarkKind::Prompt)
            .map(|mark| mark.absolute_line)
            .collect();
        prompt_lines.sort_unstable();
        (
            snapshot.color_lines.len().max(1),
            usize::from(pane.terminal.cols),
            snapshot.total_scrollback_rows,
            row_text,
            prompt_lines,
        )
    };
    let grid = CopyModeGrid {
        rows,
        cols,
        total_scrollback_rows,
        cursor_row_text: &row_text,
        prompt_lines: &prompt_lines,
    };

    let action = ctx
        .state
        .copy_mode
        .as_mut()
        .map(|copy| copy.navigation.handle_key(key, grid));
    match action {
        Some(CopyModeAction::RequestCopy) => (true, exit(ctx, true)),
        Some(CopyModeAction::Cancel) => (true, exit(ctx, false)),
        Some(CopyModeAction::Moved | CopyModeAction::SelectionChanged) => {
            let offset = ctx
                .state
                .copy_mode
                .as_ref()
                .map(|copy| copy.navigation.scrollback_offset())
                .unwrap_or(0);
            if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
                pane.terminal.set_scrollback(offset);
            }
            (true, Update::full())
        }
        Some(CopyModeAction::Ignored) | None => (true, Update::none()),
    }
}

fn cycle_copy_search(ctx: &mut Context<HyprmuxApp>, backward: bool) -> Update {
    let Some(copy) = ctx.state.copy_mode.as_mut() else {
        return Update::none();
    };
    if copy.search_matches.is_empty() {
        return Update::full();
    }
    let len = copy.search_matches.len();
    copy.search_current = if backward {
        copy.search_current.checked_sub(1).unwrap_or(len - 1)
    } else {
        (copy.search_current + 1) % len
    };
    let matched = copy.search_matches[copy.search_current].clone();
    apply_copy_search_match(ctx, &matched);
    Update::full()
}

pub(crate) fn apply_copy_search_match(
    ctx: &mut Context<HyprmuxApp>,
    matched: &crate::state::CopySearchMatch,
) {
    let Some(copy) = ctx.state.copy_mode.as_mut() else {
        return;
    };
    copy.navigation
        .goto(matched.line, matched.start_col, matched.offset);
    let target = copy.target;
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(matched.offset);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui_lipan::TestBackend;

    #[test]
    fn framework_navigation_selection_keeps_the_cursor_cell_as_endpoint() {
        let mut navigation = TerminalCopyMode::new(2, 4, 0);
        let grid = CopyModeGrid {
            rows: 5,
            cols: 20,
            total_scrollback_rows: 0,
            cursor_row_text: "",
            prompt_lines: &[],
        };
        let _ = navigation.handle_key(
            KeyEvent {
                code: KeyCode::Char('v'),
                mods: KeyMods::NONE,
            },
            grid,
        );
        navigation.goto(1, 2, 0);
        let selection = navigation
            .selection(0)
            .expect("anchor should create a selection");
        assert_eq!((selection.anchor.line, selection.anchor.col), (2, 4));
        assert_eq!((selection.cursor.line, selection.cursor.col), (1, 2));
    }

    #[test]
    fn framework_navigation_uses_display_columns_for_wide_text() {
        let mut navigation = TerminalCopyMode::new(0, 0, 0);
        let action = navigation.handle_key(
            KeyEvent {
                code: KeyCode::Char('w'),
                mods: KeyMods::NONE,
            },
            CopyModeGrid {
                rows: 1,
                cols: 20,
                total_scrollback_rows: 0,
                cursor_row_text: "界 foo",
                prompt_lines: &[],
            },
        );
        assert_eq!(action, CopyModeAction::Moved);
        assert_eq!(navigation.cursor(), (0, 3));
    }

    #[test]
    fn copy_feedback_expiry_returns_scrollback_live_and_rejects_stale_timers() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                let mut backend = TestBackend::new(HyprmuxApp::default());
                let target = backend
                    .state()
                    .current()
                    .focused_pane
                    .expect("focused pane");
                let pane = find_pane_mut(backend.state_mut(), target).expect("pane");
                let output = (0..48)
                    .map(|row| format!("line {row}\r\n"))
                    .collect::<String>();
                pane.terminal.process_server_output(output.as_bytes());
                assert!(pane.terminal.set_scrollback(2));
                backend.state_mut().copy_feedback_target = Some((0, target));
                backend.state_mut().copy_feedback_epoch = 7;

                backend
                    .dispatch(crate::Msg::CopyFeedbackExpired(0, target, 6))
                    .expect("stale expiry");
                assert_eq!(
                    find_pane_mut(backend.state_mut(), target)
                        .expect("pane")
                        .terminal
                        .scrollback_offset(),
                    2
                );

                backend
                    .dispatch(crate::Msg::CopyFeedbackExpired(0, target, 7))
                    .expect("current expiry");
                assert_eq!(
                    find_pane_mut(backend.state_mut(), target)
                        .expect("pane")
                        .terminal
                        .scrollback_offset(),
                    0
                );
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread completes");
    }
}
