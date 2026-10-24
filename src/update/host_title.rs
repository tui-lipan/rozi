//! The host terminal's window title, kept in sync with the session and the focused pane.
//!
//! Recomputed after every full update, every routed key, and every focused-pane title change.
//! The framework writes OSC 2 only when the text differs from what the host already shows, so
//! recomputing an unchanged title costs a few small allocations and nothing on the wire.

use tui_lipan::prelude::*;

use crate::AppRoot;
use crate::state::State;

/// Shown when every placeholder in the format came out empty.
const FALLBACK: &str = "rozi";

pub(crate) fn sync(ctx: &mut Context<AppRoot>) {
    if ctx.state.config.window_title.is_empty() {
        return;
    }
    let title = window_title(&ctx.state);
    ctx.set_title(title);
}

/// The startup title, before any session or pane exists. `None` when the user turned titles off.
pub(crate) fn initial(config: &crate::config::Config) -> Option<&'static str> {
    (!config.window_title.is_empty()).then_some(FALLBACK)
}

fn window_title(state: &State) -> String {
    let pane = state
        .focused_pane_target()
        .and_then(|(id, local)| crate::pane::lifecycle::find_pane_in_namespace(state, id, local));
    let remote_attached = state.current().remote_target.is_some();
    let title = render(&state.config.window_title, |name| match name {
        "session" => state.named_session().map(str::to_string),
        "pane" => pane.map(|pane| strip_spinner(&pane.titlebar_title(remote_attached)).to_string()),
        "cwd" => pane.and_then(|pane| pane_cwd(pane, remote_attached)),
        "host" => crate::platform::user::hostname(),
        _ => None,
    });
    if title.is_empty() {
        FALLBACK.to_string()
    } else {
        title
    }
}

fn pane_cwd(pane: &crate::state::Pane, remote_attached: bool) -> Option<String> {
    if let Some(path) = &pane.terminal.display_path {
        return Some(path.clone());
    }
    let cwd = pane.live_cwd().or_else(|| pane.identity.cwd.clone())?;
    Some(if remote_attached {
        cwd
    } else {
        crate::platform::paths::compress_home(&cwd)
    })
}

/// Drop the braille spinner agents animate at the front of their title while a turn runs, so the
/// host title does not redraw on every spinner frame.
fn strip_spinner(title: &str) -> &str {
    let mut chars = title.chars();
    match chars.next() {
        Some(ch) if ('\u{2800}'..='\u{28ff}').contains(&ch) => {
            chars.as_str().trim_start_matches([' ', '-', '·'])
        }
        _ => title,
    }
}

/// Expand `{name}` placeholders through `value`. An empty or unknown-valued placeholder drops the
/// text after it up to the next placeholder, and a separator left at either end is trimmed, so
/// `rozi · {session} · {pane}` reads `rozi · <pane>` in a temporary session. Text in braces that
/// `value` does not recognize stays as written.
fn render(format: &str, value: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    let mut skip_literal = false;
    while !rest.is_empty() {
        let (literal, after) = match rest.find('{') {
            Some(start) => rest.split_at(start),
            None => (rest, ""),
        };
        if !skip_literal {
            out.push_str(literal);
        }
        skip_literal = false;
        rest = after;
        if rest.is_empty() {
            break;
        }
        let Some(end) = rest.find('}') else {
            out.push_str(rest);
            break;
        };
        let name = &rest[1..end];
        if is_placeholder(name) {
            match value(name).filter(|text| !text.trim().is_empty()) {
                Some(text) => out.push_str(text.trim()),
                None => skip_literal = true,
            }
        } else {
            out.push_str(&rest[..=end]);
        }
        rest = &rest[end + 1..];
    }
    out.trim_matches(|ch: char| ch.is_whitespace() || SEPARATORS.contains(&ch))
        .to_string()
}

const PLACEHOLDERS: &[&str] = &["session", "pane", "cwd", "host"];
const SEPARATORS: &[char] = &['·', '—', '–', '-', '|', ':', ','];

fn is_placeholder(name: &str) -> bool {
    PLACEHOLDERS.contains(&name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn values<'a>(pairs: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<String> + 'a {
        move |name| {
            pairs
                .iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        }
    }

    #[test]
    fn expands_every_placeholder() {
        let title = render(
            "{session} · {pane} · {cwd} @ {host}",
            values(&[
                ("session", "dev"),
                ("pane", "nvim"),
                ("cwd", "~/src"),
                ("host", "box"),
            ]),
        );
        assert_eq!(title, "dev · nvim · ~/src @ box");
    }

    #[test]
    fn an_empty_placeholder_takes_its_separator_along() {
        let format = crate::config::DEFAULT_WINDOW_TITLE;
        assert_eq!(
            render(format, values(&[("session", "dev"), ("pane", "~/src")])),
            "rozi · dev · ~/src"
        );
        assert_eq!(render(format, values(&[("pane", "~/src")])), "rozi · ~/src");
        assert_eq!(render(format, values(&[("session", "dev")])), "rozi · dev");
        assert_eq!(render(format, values(&[])), "rozi");
        assert_eq!(
            render("{session} — {pane}", values(&[("pane", "htop")])),
            "htop"
        );
        assert_eq!(render("{session}", values(&[("session", "  ")])), "");
    }

    #[test]
    fn unknown_braces_stay_literal() {
        assert_eq!(
            render("{pane} {nope} {", values(&[("pane", "htop")])),
            "htop {nope} {"
        );
    }

    fn state_with_focused_title(title: &str) -> State {
        let mut state = State::new(crate::config::Config::default(), Theme::default());
        let pane = &mut state.current_mut().workspaces[0].panes[0];
        pane.terminal.title = Some(title.to_string());
        pane.identity.cwd = None;
        let id = pane.id;
        state.set_focused_pane(Some(id));
        state
    }

    #[test]
    fn follows_the_named_session_and_focused_pane() {
        let mut state = state_with_focused_title("⠙ Refactor the parser");
        assert_eq!(window_title(&state), "rozi · Refactor the parser");

        state.current_mut().session_name = Some("eph-42".into());
        state.current_mut().session_attached = true;
        assert_eq!(window_title(&state), "rozi · Refactor the parser");

        state.current_mut().session_name = Some("dev".into());
        assert_eq!(window_title(&state), "rozi · dev · Refactor the parser");

        state.config.window_title = "{pane}".into();
        state.set_focused_pane(None);
        assert_eq!(window_title(&state), FALLBACK);
    }

    #[test]
    fn an_empty_format_turns_titles_off() {
        let mut config = crate::config::Config::default();
        assert_eq!(initial(&config), Some(FALLBACK));
        config.window_title.clear();
        assert_eq!(initial(&config), None);
    }

    #[test]
    fn strips_a_leading_agent_spinner() {
        assert_eq!(
            strip_spinner("⠧ - Waiting for response…"),
            "Waiting for response…"
        );
        assert_eq!(
            strip_spinner("⠙ Refactor the parser"),
            "Refactor the parser"
        );
        assert_eq!(strip_spinner("nvim ⠙"), "nvim ⠙");
    }
}
