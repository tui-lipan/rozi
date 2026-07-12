use std::sync::OnceLock;

use regex_lite::Regex;
use tui_lipan::prelude::*;

use crate::HyprmuxApp;
use crate::ops::focus::request_current_pane_focus;
use crate::pane_lifecycle::find_pane;
use crate::state::{HintModeState, Mode};

const LABEL_KEYS: &[u8] = b"asdfghjkl;";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Url,
    Path,
    GitSha,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintMatch {
    pub row: usize,
    pub start_col: usize,
    pub end_col: usize,
    pub text: String,
    pub kind: HintKind,
}

pub fn scan_snapshot(text: &str) -> Vec<HintMatch> {
    static URL: OnceLock<Regex> = OnceLock::new();
    static PATH: OnceLock<Regex> = OnceLock::new();
    static SHA: OnceLock<Regex> = OnceLock::new();
    let patterns = [
        (
            HintKind::Url,
            URL.get_or_init(|| Regex::new(r"https?://[^\s<>]+").unwrap()),
        ),
        (
            HintKind::Path,
            PATH.get_or_init(|| Regex::new(r"(?:\.?\.?/|~/|/)[^\s:]+(?:[:][0-9]+)?").unwrap()),
        ),
        (
            HintKind::GitSha,
            SHA.get_or_init(|| Regex::new(r"\b[0-9a-fA-F]{7,40}\b").unwrap()),
        ),
    ];
    let mut out = Vec::new();
    for (row, line) in text.lines().enumerate() {
        for (kind, regex) in patterns {
            for matched in regex.find_iter(line) {
                let raw = matched.as_str();
                let trimmed = raw.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '}']);
                if trimmed.is_empty() {
                    continue;
                }
                // A pure-decimal run (timestamp, PID, byte count) is far more likely than a
                // letterless SHA; require at least one hex letter to keep numeric output quiet.
                if kind == HintKind::GitSha && !trimmed.bytes().any(|b| b.is_ascii_alphabetic()) {
                    continue;
                }
                let start_col = line[..matched.start()].chars().count();
                let end_col = start_col + trimmed.chars().count();
                if out.iter().any(|existing: &HintMatch| {
                    existing.row == row
                        && start_col < existing.end_col
                        && end_col > existing.start_col
                }) {
                    continue;
                }
                out.push(HintMatch {
                    row,
                    start_col,
                    end_col,
                    text: trimmed.to_string(),
                    kind,
                });
            }
        }
    }
    out.sort_by_key(|matched| (matched.row, matched.start_col));
    out
}

pub fn hint_labels(n: usize) -> Vec<String> {
    if n <= LABEL_KEYS.len() {
        return LABEL_KEYS[..n]
            .iter()
            .map(|key| char::from(*key).to_string())
            .collect();
    }
    (0..n)
        .map(|index| {
            let first = LABEL_KEYS[(index / LABEL_KEYS.len()) % LABEL_KEYS.len()];
            let second = LABEL_KEYS[index % LABEL_KEYS.len()];
            format!("{}{}", char::from(first), char::from(second))
        })
        .collect()
}

pub(crate) fn enter(ctx: &mut Context<HyprmuxApp>) -> Update {
    let Some(target) = ctx.state.focused_pane else {
        return Update::full();
    };
    let Some(pane) = find_pane(&ctx.state, target) else {
        return Update::full();
    };
    let matches = scan_snapshot(&pane.terminal.capture_text());
    if matches.is_empty() {
        ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            "No hints in this pane",
        ));
        return Update::full();
    }
    let offset = pane.terminal.scrollback_offset();
    let labels = hint_labels(matches.len());
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

fn exit(ctx: &mut Context<HyprmuxApp>) -> Update {
    ctx.state.hint_mode = None;
    ctx.state.mode = Mode::Normal;
    ctx.state.commands_dirty = true;
    request_current_pane_focus(ctx);
    Update::full()
}

pub(crate) fn handle_hint_key(ctx: &mut Context<HyprmuxApp>, key: KeyEvent) -> (bool, Update) {
    if key.is(KeyCode::Esc) || matches!(key.code, KeyCode::Char('q' | 'Q')) {
        return (true, exit(ctx));
    }
    let KeyCode::Char(ch) = key.code else {
        return (true, Update::none());
    };
    let lower = ch.to_ascii_lowercase();
    if !LABEL_KEYS.contains(&(lower as u8)) {
        return (true, Update::none());
    }
    let Some(state) = ctx.state.hint_mode.as_mut() else {
        return (true, exit(ctx));
    };
    state.input.push(lower);
    let candidates: Vec<usize> = state
        .labels
        .iter()
        .enumerate()
        .filter(|(_, label)| label.starts_with(&state.input))
        .map(|(index, _)| index)
        .collect();
    if candidates.len() != 1 || state.labels[candidates[0]] != state.input {
        return (true, Update::full());
    }
    let matched = state.matches[candidates[0]].clone();
    let open = ch.is_ascii_uppercase() && matched.kind == HintKind::Url;
    let result = if open {
        tui_lipan::utils::open_url(&matched.text).map_err(|err| err.to_string())
    } else {
        ctx.clipboard()
            .copy(&matched.text)
            .map_err(|err| err.to_string())
    };
    match result {
        Ok(()) => ctx.toast().push(crate::pty_events::info_toast(
            &ctx.state.theme,
            if open { "Opened hint" } else { "Copied hint" },
        )),
        Err(error) => ctx.toast().push(crate::pty_events::error_toast(
            &ctx.state.theme,
            "Hint failed",
            error,
        )),
    };
    (true, exit(ctx))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_and_deduplicates_hints() {
        let found = scan_snapshot("https://example.com/a). ./src/main.rs:12 deadbeef");
        assert_eq!(
            found.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["https://example.com/a", "./src/main.rs:12", "deadbeef"]
        );
        assert_eq!(found[0].kind, HintKind::Url);
    }

    #[test]
    fn pure_decimal_runs_are_not_sha_hints() {
        assert!(scan_snapshot("size 17520384 pid 1234567890").is_empty());
        let found = scan_snapshot("rev 1234abc timestamp 1720780800");
        assert_eq!(
            found.iter().map(|m| m.text.as_str()).collect::<Vec<_>>(),
            vec!["1234abc"]
        );
    }

    #[test]
    fn labels_are_unique_and_prefix_free() {
        for n in [1, 10, 11] {
            let labels = hint_labels(n);
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
}
