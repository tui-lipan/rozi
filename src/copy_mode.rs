use std::time::Duration;

use tui_lipan::prelude::*;
use tui_lipan::text_motion::{
    big_word_backward_start, big_word_end, big_word_forward_start, first_nonblank_in_line,
    word_backward_start, word_end, word_forward_start,
};

use crate::HyprmuxApp;
use crate::ops::focus::request_current_pane_focus;
use crate::pane_lifecycle::find_pane_mut;
use crate::state::{CopyFlashState, CopyModeState, Mode, PaneId};

/// Apply a `tui_lipan::text_motion` byte-offset motion to a char-index column, since
/// copy-mode columns (like the rest of the pane's grid coordinates) are plain char counts.
fn motion_col(row_text: &str, col: usize, motion: fn(&str, usize) -> usize) -> usize {
    byte_to_col(row_text, motion(row_text, col_to_byte(row_text, col)))
}

/// Like [`motion_col`], for the `e`/`E` "word end" motions. Unlike `w`/`b`, which land on a run
/// boundary the same way under either convention, `word_end`/`big_word_end` are defined in terms
/// of a text-editor insertion-point cursor (the gap *after* a character): called with the cell
/// column directly, a cursor already on a word's last character looks indistinguishable from one
/// mid-word and won't advance to the next word on repeat presses. Feeding in the insertion point
/// just after the current cell (matching the "gap after a char" convention) and mapping the
/// resulting offset back down to the char before it reproduces real vim's `e`/`E`.
fn motion_col_end(row_text: &str, col: usize, motion: fn(&str, usize) -> usize) -> usize {
    let after_col_byte = row_text
        .char_indices()
        .nth(col)
        .map(|(byte, ch)| byte + ch.len_utf8())
        .unwrap_or(row_text.len());
    byte_to_col(row_text, motion(row_text, after_col_byte)).saturating_sub(1)
}

fn col_to_byte(text: &str, col: usize) -> usize {
    text.char_indices()
        .nth(col)
        .map(|(byte, _)| byte)
        .unwrap_or(text.len())
}

fn byte_to_col(text: &str, byte_offset: usize) -> usize {
    text[..byte_offset.min(text.len())].chars().count()
}

/// Enter copy mode on the focused pane: seed the cursor at the live cursor position with no
/// selection, and park scrollback at its current offset. Closes any open overlay first.
pub(crate) fn enter(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.focused_pane else {
        return Update::full();
    };
    let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
        return Update::full();
    };
    let (cursor_row, cursor_col) = pane.terminal.cursor_position();
    let offset = pane.terminal.scrollback_offset();

    ctx.state.copy_mode = Some(CopyModeState {
        target,
        cursor_row,
        cursor_col,
        anchor: None,
        offset,
        search_matches: Vec::new(),
        search_current: 0,
    });
    ctx.state.mode = Mode::Copy;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    Update::full()
}

/// Leave copy mode. When `copy` is set the current selection (if any) is sent to the system
/// clipboard. Either way scrollback snaps back to the live view and focus returns to the pane.
pub(crate) fn exit(ctx: &mut Context<HyprmuxApp>, copy: bool) -> Update {
    let Some(state) = ctx.state.copy_mode.take() else {
        ctx.state.mode = Mode::Normal;
        ctx.state.commands_dirty = true;
        return Update::full();
    };

    let mut copied_selection = None;
    if copy
        && let Some((anchor, cursor)) = state.selection()
        && let Some(pane) = find_pane_mut(&mut ctx.state, state.target)
    {
        let text = pane.terminal.extract_text(anchor, cursor);
        if !text.is_empty() {
            match ctx.clipboard().copy(&text) {
                // The copy flash below is the confirmation; a "Copied" toast would double it.
                Ok(()) => copied_selection = Some((anchor, cursor)),
                Err(err) => {
                    ctx.toast().push(crate::pty_events::error_toast(
                        &ctx.state.theme,
                        "Copy failed",
                        err.to_string(),
                    ));
                }
            }
        }
    }

    if copied_selection.is_none()
        && let Some(pane) = find_pane_mut(&mut ctx.state, state.target)
    {
        pane.terminal.set_scrollback(0);
    }
    ctx.state.mode = Mode::Normal;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    copied_selection.map_or_else(Update::full, |selection| {
        start_copy_flash(ctx, state.target, selection, true)
    })
}

pub(crate) fn start_copy_flash(
    ctx: &mut Context<HyprmuxApp>,
    target: PaneId,
    selection: ((usize, usize), (usize, usize)),
    return_to_live: bool,
) -> Update {
    let id = ctx.state.next_copy_flash_id;
    ctx.state.next_copy_flash_id = ctx.state.next_copy_flash_id.saturating_add(1);
    ctx.state.copy_flash = Some(CopyFlashState {
        id,
        target,
        selection,
        return_to_live,
        clearing: false,
    });
    Update::with_command(copy_flash_timer(target, id))
}

pub(crate) fn expire_flash(ctx: &mut Context<HyprmuxApp>, target: PaneId, id: u64) -> Update {
    if !ctx
        .state
        .copy_flash
        .is_some_and(|flash| flash.target == target && flash.id == id)
    {
        return Update::none();
    }
    let return_to_live = ctx
        .state
        .copy_flash
        .is_some_and(|flash| flash.return_to_live);
    if let Some(flash) = ctx.state.copy_flash.as_mut() {
        flash.clearing = true;
    }
    if return_to_live && let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(0);
    }
    Update::full()
}

fn copy_flash_timer(target: PaneId, id: u64) -> Command {
    Command::spawn(move |link: CommandLink<crate::Msg>| {
        std::thread::sleep(copy_flash_duration());
        link.send(crate::Msg::CopyFlashExpired(target, id));
    })
}

fn copy_flash_duration() -> Duration {
    Duration::from_millis(ClipboardConfig::default().copy_feedback_duration_ms as u64)
}

/// Route a key while in copy mode. Returns `(handled, update)`; every key is consumed so
/// nothing leaks to the PTY, mirroring resize mode.
pub(crate) fn handle_copy_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Esc) || key.is(KeyCode::Char('q')) {
        return (true, exit(ctx, false));
    }
    if key.is(KeyCode::Char('y')) || key.is(KeyCode::Enter) {
        return (true, exit(ctx, true));
    }
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
    if key.is(KeyCode::Char('[')) {
        return (true, jump_semantic_prompt(ctx, false));
    }
    if key.is(KeyCode::Char(']')) {
        return (true, jump_semantic_prompt(ctx, true));
    }
    if key.is(KeyCode::Char('o')) {
        return (true, crate::ops::last_output::copy_last_output(ctx));
    }
    if key.is(KeyCode::Char('v')) || key.is(KeyCode::Char(' ')) {
        if let Some(copy) = ctx.state.copy_mode.as_mut() {
            copy.anchor = Some((copy.cursor_row, copy.cursor_col));
        }
        return (true, Update::full());
    }

    let (cols, rows, total, row_text) = {
        let Some(target) = ctx.state.copy_mode.as_ref().map(|copy| copy.target) else {
            return (true, Update::none());
        };
        let cursor_row = ctx
            .state
            .copy_mode
            .as_ref()
            .map(|copy| copy.cursor_row)
            .unwrap_or(0);
        let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
            return (true, Update::none());
        };
        (
            usize::from(pane.terminal.cols),
            usize::from(pane.terminal.rows),
            pane.terminal.total_scrollback_rows(),
            pane.terminal.row_text(cursor_row),
        )
    };

    let Some(copy) = ctx.state.copy_mode.as_mut() else {
        return (true, Update::none());
    };

    let half_page = (rows / 2).max(1);
    match key.code {
        KeyCode::Char('h' | 'H') | KeyCode::Left => {
            copy.cursor_col = copy.cursor_col.saturating_sub(1);
        }
        KeyCode::Char('l' | 'L') | KeyCode::Right => {
            copy.cursor_col = (copy.cursor_col + 1).min(cols.saturating_sub(1));
        }
        KeyCode::Char('k' | 'K') | KeyCode::Up => move_up(copy, 1, total),
        KeyCode::Char('j' | 'J') | KeyCode::Down => move_down(copy, 1, rows),
        KeyCode::Char('u') if key.mods.ctrl => move_up(copy, half_page, total),
        KeyCode::Char('d') if key.mods.ctrl => move_down(copy, half_page, rows),
        KeyCode::Char('g') => {
            copy.offset = total;
            copy.cursor_row = 0;
        }
        KeyCode::Char('G') => {
            copy.offset = 0;
            copy.cursor_row = rows.saturating_sub(1);
        }
        KeyCode::Char('w') => {
            copy.cursor_col = motion_col(&row_text, copy.cursor_col, word_forward_start);
        }
        KeyCode::Char('b') => {
            copy.cursor_col = motion_col(&row_text, copy.cursor_col, word_backward_start);
        }
        KeyCode::Char('e') => {
            copy.cursor_col = motion_col_end(&row_text, copy.cursor_col, word_end);
        }
        KeyCode::Char('W') => {
            copy.cursor_col = motion_col(&row_text, copy.cursor_col, big_word_forward_start);
        }
        KeyCode::Char('B') => {
            copy.cursor_col = motion_col(&row_text, copy.cursor_col, big_word_backward_start);
        }
        KeyCode::Char('E') => {
            copy.cursor_col = motion_col_end(&row_text, copy.cursor_col, big_word_end);
        }
        KeyCode::Char('0') => copy.cursor_col = 0,
        KeyCode::Char('^') => {
            copy.cursor_col = byte_to_col(
                &row_text,
                first_nonblank_in_line(&row_text, 0, row_text.len()),
            );
        }
        KeyCode::Char('$') => {
            copy.cursor_col = row_text.chars().count().saturating_sub(1);
        }
        _ => return (true, Update::none()),
    }

    let offset = copy.offset;
    let target = copy.target;
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(offset);
    }
    (true, Update::full())
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

fn jump_semantic_prompt(ctx: &mut Context<HyprmuxApp>, forward: bool) -> Update {
    let Some(copy) = ctx.state.copy_mode.as_ref() else {
        return Update::none();
    };
    let target = copy.target;
    let cursor_row = copy.cursor_row;
    let offset = copy.offset;
    let (prompts, history) = {
        let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
            return Update::none();
        };
        let prompts: Vec<_> = pane
            .terminal
            .semantic_marks()
            .into_iter()
            .filter(|mark| mark.kind == tui_lipan::prelude::SemanticMarkKind::Prompt)
            .collect();
        (prompts, pane.terminal.total_scrollback_rows())
    };
    if prompts.is_empty() {
        return Update::full();
    }
    // Absolute line of the copy cursor: history lines above the viewport, minus
    // scrollback offset, plus the cursor's row within the viewport.
    let current_abs = history.saturating_sub(offset).saturating_add(cursor_row);
    let idx = if forward {
        prompts
            .iter()
            .position(|mark| mark.absolute_line > current_abs)
            .unwrap_or(0)
    } else {
        prompts
            .iter()
            .rposition(|mark| mark.absolute_line < current_abs)
            .unwrap_or(prompts.len() - 1)
    };
    let mark = prompts[idx];
    let Some((new_offset, row)) = ({
        let Some(pane) = find_pane_mut(&mut ctx.state, target) else {
            return Update::full();
        };
        pane.terminal.absolute_line_to_viewport(mark.absolute_line)
    }) else {
        return Update::full();
    };
    if let Some(copy) = ctx.state.copy_mode.as_mut() {
        copy.offset = new_offset;
        copy.cursor_row = row;
        copy.cursor_col = 0;
    }
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(new_offset);
    }
    Update::full()
}

pub(crate) fn apply_copy_search_match(
    ctx: &mut Context<HyprmuxApp>,
    matched: &crate::state::CopySearchMatch,
) {
    let Some(copy) = ctx.state.copy_mode.as_mut() else {
        return;
    };
    copy.offset = matched.offset;
    copy.cursor_row = matched.line;
    copy.cursor_col = matched.start_col;
    let target = copy.target;
    let offset = copy.offset;
    if let Some(pane) = find_pane_mut(&mut ctx.state, target) {
        pane.terminal.set_scrollback(offset);
    }
}

/// Move the cursor up `steps` rows; at the top of the viewport, scroll further into history
/// (raising the scrollback offset, clamped to the total) while keeping the cursor on row 0.
fn move_up(copy: &mut CopyModeState, steps: usize, total: usize) {
    for _ in 0..steps {
        if copy.cursor_row > 0 {
            copy.cursor_row -= 1;
        } else if copy.offset < total {
            copy.offset += 1;
        } else {
            break;
        }
    }
}

/// Move the cursor down `steps` rows; at the bottom of the viewport, scroll toward the live
/// view (lowering the offset) while keeping the cursor on the bottom row.
fn move_down(copy: &mut CopyModeState, steps: usize, rows: usize) {
    let bottom = rows.saturating_sub(1);
    for _ in 0..steps {
        if copy.cursor_row < bottom {
            copy.cursor_row += 1;
        } else if copy.offset > 0 {
            copy.offset -= 1;
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::CopyModeState;

    fn copy_state() -> CopyModeState {
        CopyModeState {
            target: 1,
            cursor_row: 0,
            cursor_col: 0,
            anchor: None,
            offset: 0,
            search_matches: Vec::new(),
            search_current: 0,
        }
    }

    #[test]
    fn move_up_scrolls_history_at_top_edge() {
        let mut copy = copy_state();
        copy.cursor_row = 2;
        move_up(&mut copy, 1, 10);
        assert_eq!(copy.cursor_row, 1);
        move_up(&mut copy, 2, 10);
        assert_eq!((copy.cursor_row, copy.offset), (0, 1));
        // Clamped to total scrollback rows.
        move_up(&mut copy, 100, 10);
        assert_eq!(copy.offset, 10);
    }

    #[test]
    fn move_down_scrolls_toward_live_at_bottom_edge() {
        let mut copy = copy_state();
        copy.offset = 5;
        copy.cursor_row = 0;
        move_down(&mut copy, 100, 4);
        // Cursor pinned to bottom row, offset drained back to the live view.
        assert_eq!((copy.cursor_row, copy.offset), (3, 0));
    }

    #[test]
    fn copy_search_match_maps_onto_cursor_and_preserves_anchor() {
        let mut copy = copy_state();
        copy.anchor = Some((1, 2));
        copy.search_matches = vec![
            crate::state::CopySearchMatch {
                offset: 5,
                line: 3,
                start_col: 4,
                end_col: 9,
            },
            crate::state::CopySearchMatch {
                offset: 2,
                line: 1,
                start_col: 0,
                end_col: 3,
            },
        ];
        let matched = copy.search_matches[0].clone();
        copy.offset = matched.offset;
        copy.cursor_row = matched.line;
        copy.cursor_col = matched.start_col;
        assert_eq!((copy.offset, copy.cursor_row, copy.cursor_col), (5, 3, 4));
        assert_eq!(copy.anchor, Some((1, 2)));

        let matched = copy.search_matches[1].clone();
        copy.offset = matched.offset;
        copy.cursor_row = matched.line;
        copy.cursor_col = matched.start_col;
        assert_eq!((copy.offset, copy.cursor_row, copy.cursor_col), (2, 1, 0));
        assert_eq!(copy.anchor, Some((1, 2)));
    }

    #[test]
    fn col_byte_conversion_round_trips_through_multibyte_text() {
        let text = "héllo wörld";
        for col in 0..=text.chars().count() {
            let byte = col_to_byte(text, col);
            assert_eq!(byte_to_col(text, byte), col);
        }
    }

    #[test]
    fn motion_col_moves_by_word_forward_backward_and_end() {
        let text = "one two  three";
        // Columns are char indices: "one two  three"
        //                             0123456789...
        assert_eq!(motion_col(text, 0, word_forward_start), 4);
        assert_eq!(motion_col(text, 4, word_forward_start), 9);
        assert_eq!(motion_col(text, 9, word_backward_start), 4);
        assert_eq!(motion_col_end(text, 0, word_end), 2);
        assert_eq!(motion_col_end(text, 2, word_end), 6);
    }

    #[test]
    fn motion_col_end_advances_to_the_next_word_when_already_at_a_word_end() {
        let text = "one two  three";
        // Already on the last char of "one" (col 2): `e` should jump to the end of "two", not
        // stay put, matching real vim (repeated `e` presses keep advancing).
        assert_eq!(motion_col_end(text, 2, word_end), 6);
        // And from there, to the end of "three".
        assert_eq!(motion_col_end(text, 6, word_end), 13);
    }

    #[test]
    fn motion_col_moves_by_big_word_across_punctuation() {
        let text = "foo.bar  baz";
        assert_eq!(motion_col(text, 0, big_word_forward_start), 9);
        assert_eq!(motion_col_end(text, 0, big_word_end), 6);
        assert_eq!(motion_col(text, 9, big_word_backward_start), 0);
    }

    #[test]
    fn line_boundary_motions_use_columns_not_bytes() {
        let text = "  one two";
        assert_eq!(
            byte_to_col(text, first_nonblank_in_line(text, 0, text.len())),
            2
        );
        assert_eq!(text.chars().count().saturating_sub(1), 8);
    }
}
