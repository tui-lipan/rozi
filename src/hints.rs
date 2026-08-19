use std::ops::Range;

use tui_lipan::prelude::*;
use tui_lipan::utils::hints::{
    HOME_ROW_HINT_KEYS, HintFilter, HintScan, assign_labels, filter_labels,
};

pub use tui_lipan::utils::hints::{HintKind, HintMatch};

use crate::AppRoot;
use crate::ops::focus::request_current_pane_focus;
use crate::pane_lifecycle::find_pane;
use crate::state::{HintModeState, Mode};

/// Scan one visible snapshot for hints, rejoining soft-wrapped rows first.
///
/// `wrapped_rows` comes straight off the snapshot: without it a URL or path the terminal broke
/// across rows is scanned as the fragments it was broken into, and a fragment is rarely a hint on
/// its own. See [`tui_lipan::utils::hints::HintScan::scan_wrapped`].
pub fn scan_snapshot_with_custom(
    text: &str,
    wrapped_rows: &[bool],
    custom: &[crate::config::HintConfig],
) -> Vec<HintMatch> {
    let mut scan = HintScan::new();
    for (tag, hint) in custom.iter().enumerate() {
        let Ok(tag) = u16::try_from(tag) else {
            break;
        };
        let pattern = hint.pattern.clone();
        scan = scan.custom(tag, move |line: &str, out: &mut Vec<Range<usize>>| {
            out.extend(
                pattern
                    .find_iter(line)
                    .map(|matched| matched.start()..matched.end()),
            );
        });
    }
    scan.scan_wrapped(text, wrapped_rows)
}

fn can_open(kind: HintKind, custom: &[crate::config::HintConfig]) -> bool {
    match kind {
        HintKind::Url => true,
        HintKind::Custom(tag) => custom.get(usize::from(tag)).is_some_and(|hint| hint.open),
        HintKind::Path | HintKind::GitSha => false,
    }
}

pub(crate) fn enter(ctx: &mut Context<AppRoot>) -> Update {
    let Some(target) = ctx.state.focused_pane() else {
        return Update::full();
    };
    let Some(pane) = find_pane(&ctx.state, target) else {
        return Update::full();
    };
    let snapshot = pane.terminal.snapshot();
    let matches = scan_snapshot_with_custom(
        &snapshot.text,
        &snapshot.wrapped_rows,
        &ctx.state.config.hints,
    );
    if matches.is_empty() {
        crate::pty_events::notify_info(ctx, "No hints in this pane");
        return Update::full();
    }
    let offset = pane.terminal.scrollback_offset();
    crate::copy_mode::clear_copy_feedback(ctx);
    let labels = assign_labels(matches.len(), HOME_ROW_HINT_KEYS);
    ctx.state.hint_mode = Some(HintModeState {
        target,
        matches,
        labels,
        input: String::new(),
        offset,
    });
    ctx.state.mode = Mode::Hint;
    ctx.state.show_help = false;
    ctx.state.show_palette = false;
    ctx.state.search = None;
    Update::full()
}

/// Leave hint mode because the pointer was used, reporting whether it was up.
///
/// Hint mode labels one pane and is driven from the keyboard, so a click is not an answer to it -
/// it is a way out of it. The caller swallows the click that dismissed the mode rather than also
/// acting on it, the way any modal does.
pub(crate) fn cancel_for_pointer(ctx: &mut Context<AppRoot>) -> bool {
    if ctx.state.hint_mode.is_none() && ctx.state.mode != Mode::Hint {
        return false;
    }
    exit(ctx);
    ctx.state.consumed_pointer_click = true;
    true
}

fn exit(ctx: &mut Context<AppRoot>) -> Update {
    ctx.state.hint_mode = None;
    ctx.state.mode = Mode::Normal;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn handle_hint_key(ctx: &mut Context<AppRoot>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Esc) || matches!(key.code, KeyCode::Char('q' | 'Q')) {
        return (true, exit(ctx));
    }
    let KeyCode::Char(ch) = key.code else {
        return (true, Update::none());
    };
    let lower = ch.to_ascii_lowercase();
    if !HOME_ROW_HINT_KEYS.as_bytes().contains(&(lower as u8)) {
        return (true, Update::none());
    }
    let Some(state) = ctx.state.hint_mode.as_mut() else {
        return (true, exit(ctx));
    };
    let target = state.target;
    state.input.push(lower);
    let selected = match filter_labels(&state.labels, &state.input) {
        HintFilter::Selected(index) if state.labels[index] == state.input => index,
        HintFilter::NoMatch | HintFilter::Ambiguous | HintFilter::Selected(_) => {
            return (true, Update::full());
        }
    };
    if selected >= state.matches.len() {
        return (true, Update::full());
    }
    let matched = state.matches[selected].clone();
    let open = ch.is_ascii_uppercase() && can_open(matched.kind, &ctx.state.config.hints);
    let result = if open {
        tui_lipan::utils::open_url(&matched.text).map_err(|err| err.to_string())
    } else {
        ctx.clipboard()
            .copy(&matched.text)
            .map_err(|err| err.to_string())
    };
    let copied = result.is_ok() && !open;
    match result {
        // Success needs no toast: an opened URL raises the browser and a copy flashes below.
        Ok(()) => {}
        Err(error) => {
            crate::pty_events::notify_error(ctx, "Hint failed", error);
        }
    };
    let feedback = copied.then(|| {
        crate::copy_mode::flash_copy_feedback(
            ctx,
            target,
            tui_lipan::utils::GridSelection {
                anchor: tui_lipan::utils::GridPos {
                    row: matched.row(),
                    col: matched.start_col(),
                },
                cursor: tui_lipan::utils::GridPos {
                    row: matched.end_row(),
                    col: matched.end_col().saturating_sub(1),
                },
            },
        )
    });
    let mut update = exit(ctx);
    update.command = feedback;
    (true, update)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_and_deduplicates_hints() {
        let found = scan_snapshot_with_custom(
            "https://example.com/a). ./src/main.rs:12 deadbeef",
            &[],
            &[],
        );
        assert_eq!(
            found.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a", "./src/main.rs:12", "deadbeef"]
        );
        assert_eq!(found[0].kind, HintKind::Url);
    }

    #[test]
    fn scans_multiple_urls_after_ascii_prose() {
        let found =
            scan_snapshot_with_custom("error at https://a.test and https://b.test", &[], &[]);
        assert_eq!(
            found
                .iter()
                .map(|matched| matched.text.as_str())
                .collect::<Vec<_>>(),
            vec!["https://a.test", "https://b.test"]
        );
    }

    #[test]
    fn pure_decimal_runs_are_not_sha_hints() {
        assert!(scan_snapshot_with_custom("size 17520384 pid 1234567890", &[], &[]).is_empty());
        let found = scan_snapshot_with_custom("rev 1234abc timestamp 1720780800", &[], &[]);
        assert_eq!(
            found.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["1234abc"]
        );
    }

    #[test]
    fn labels_are_unique_and_prefix_free() {
        for n in [1, 10, 11] {
            let labels = assign_labels(n, HOME_ROW_HINT_KEYS);
            assert_eq!(labels.len(), n);
            for (i, label) in labels.iter().enumerate() {
                assert!(
                    !labels
                        .iter()
                        .enumerate()
                        .any(|(j, other)| i != j && other.starts_with(label))
                );
            }
        }
    }

    #[test]
    fn custom_hints_append_after_builtins_with_stable_label_order() {
        let custom = [crate::config::HintConfig {
            pattern: regex_lite::Regex::new(r"\b(?:[0-9]{1,3}\.){3}[0-9]{1,3}\b").unwrap(),
            open: true,
        }];
        let found = scan_snapshot_with_custom(
            "see 10.0.0.1 then https://example.com and deadbeef",
            &[],
            &custom,
        );
        let texts: Vec<_> = found.iter().map(|m| m.text.as_str()).collect();
        assert!(texts.contains(&"https://example.com"));
        assert!(texts.contains(&"deadbeef"));
        assert!(texts.contains(&"10.0.0.1"));
        assert!(
            found
                .iter()
                .any(|m| m.text == "10.0.0.1" && m.kind == HintKind::Custom(0))
        );
        assert!(
            found
                .iter()
                .any(|m| m.text == "https://example.com" && m.kind == HintKind::Url)
        );
        // Left-to-right order after sort by (row, start_col).
        assert_eq!(found[0].text, "10.0.0.1");
    }

    #[test]
    fn soft_wrapped_rows_are_scanned_as_one_hint() {
        // What the shell prints when a URL is longer than the pane is wide. Neither row is a URL.
        let text =
            "https://example.test/ssr/30000\n2660/Deals?disableNav=YES  \nbash: no such file";
        let split = scan_snapshot_with_custom(text, &[], &[]);
        let joined = scan_snapshot_with_custom(text, &[true, false, false], &[]);

        // Row by row the URL is only its first line, and the remainder is nothing at all.
        assert_eq!(
            split
                .iter()
                .map(|matched| matched.text.as_str())
                .collect::<Vec<_>>(),
            vec!["https://example.test/ssr/30000"]
        );
        assert_eq!(
            joined
                .iter()
                .map(|matched| matched.text.as_str())
                .collect::<Vec<_>>(),
            vec!["https://example.test/ssr/300002660/Deals?disableNav=YES"]
        );
        assert_eq!(joined[0].kind, HintKind::Url);
        assert_eq!(joined[0].spans.len(), 2);
        assert_eq!(joined[0].row(), 0);
        assert_eq!(joined[0].end_row(), 1);
    }

    #[test]
    fn clicking_a_pane_dismisses_hint_mode_instead_of_moving_focus() {
        std::thread::Builder::new()
            .stack_size(8 * 1024 * 1024)
            .spawn(|| {
                use crate::state::{Mode, Pane};
                use crate::{AppRoot, Msg};
                use tui_lipan::TestBackend;
                use tui_lipan::prelude::Rect;

                let mut backend = TestBackend::new(AppRoot::default());
                backend.set_viewport(Rect {
                    x: 0,
                    y: 0,
                    w: 100,
                    h: 30,
                });
                {
                    let state = backend.state_mut();
                    let rect = tui_lipan::prelude::FloatRect {
                        x: 0.0,
                        y: 0.0,
                        w: 80.0,
                        h: 24.0,
                    };
                    state.current_mut().workspaces[0].panes.clear();
                    state.current_mut().workspaces[0].tile_tree = None;
                    for id in [1, 2] {
                        state.current_mut().workspaces[0]
                            .panes
                            .push(Pane::new(id, 100, rect));
                        crate::tiling::append_tiled_window(
                            &mut state.current_mut().workspaces[0],
                            id,
                        );
                    }
                    crate::ops::focus::focus_pane(state, 1);
                    state.hint_mode = Some(HintModeState {
                        target: 1,
                        matches: Vec::new(),
                        labels: Vec::new(),
                        input: String::new(),
                        offset: 0,
                    });
                    state.mode = Mode::Hint;
                }

                backend.dispatch(Msg::FocusPane(2)).expect("click pane 2");

                assert!(backend.state().hint_mode.is_none());
                assert_eq!(backend.state().mode, Mode::Normal);
                assert_eq!(
                    backend.state().current().focused_pane,
                    Some(1),
                    "the click that dismissed hint mode must not also move focus"
                );

                // The release completing that click reaches a pane running mouse tracking. It is
                // consumed with the press: forwarded, it would hand pane 2 the focus and the child
                // a button-up it never saw pressed.
                backend
                    .dispatch(Msg::PaneMouse(2, b"\x1b[<0;1;1m".to_vec()))
                    .expect("click release");
                assert_eq!(backend.state().current().focused_pane, Some(1));
                assert!(!backend.state().consumed_pointer_click);

                // With the mode gone the same click focuses normally.
                backend.dispatch(Msg::FocusPane(2)).expect("click again");
                assert_eq!(backend.state().current().focused_pane, Some(2));
            })
            .expect("spawn test thread")
            .join()
            .expect("test thread panicked");
    }

    #[test]
    fn scanners_report_display_columns_after_wide_text() {
        let found = scan_snapshot_with_custom("你 https://example.com", &[], &[]);
        assert_eq!(found[0].start_col(), 3);
    }
}
