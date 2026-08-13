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

pub fn scan_snapshot_with_custom(
    text: &str,
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
    let mut found = scan.scan(text);
    found.sort_by_key(|matched| (matched.row, matched.start_col, matched.end_col));
    found
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
    let matches = scan_snapshot_with_custom(&pane.terminal.capture_text(), &ctx.state.config.hints);
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
                    row: matched.row,
                    col: matched.start_col,
                },
                cursor: tui_lipan::utils::GridPos {
                    row: matched.row,
                    col: matched.end_col.saturating_sub(1),
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
        let found =
            scan_snapshot_with_custom("https://example.com/a). ./src/main.rs:12 deadbeef", &[]);
        assert_eq!(
            found.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a", "./src/main.rs:12", "deadbeef"]
        );
        assert_eq!(found[0].kind, HintKind::Url);
    }

    #[test]
    fn scans_multiple_urls_after_ascii_prose() {
        let found = scan_snapshot_with_custom("error at https://a.test and https://b.test", &[]);
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
        assert!(scan_snapshot_with_custom("size 17520384 pid 1234567890", &[]).is_empty());
        let found = scan_snapshot_with_custom("rev 1234abc timestamp 1720780800", &[]);
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
    fn scanners_report_display_columns_after_wide_text() {
        let found = scan_snapshot_with_custom("你 https://example.com", &[]);
        assert_eq!(found[0].start_col, 3);
    }
}
