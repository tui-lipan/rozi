//! What an agent's work is called, from the two places an agent names it: the title of the row it
//! published, and the terminal title it sets.
//!
//! The session server and every client derive these the same way, so a run reads the same in
//! Activity, the Agents tab, the Agents view, and a host monitor's summary of a session nothing is
//! attached to.

use crate::platform::paths::path_segments;

/// A task as rows present it: without the status glyph or the agent's own label prefix an agent
/// decorates its titles with, and `None` when nothing is left that the agent's name does not
/// already say.
pub fn task_title(task: &str, label: &str) -> Option<String> {
    let title = strip_agent_title_prefix(strip_title_decoration(task), label);
    (!title.is_empty() && !title.eq_ignore_ascii_case(label)).then(|| title.to_string())
}

/// What an agent's terminal title names, when it says more than the agent's name or the directory
/// it runs in: agents write their current work there, which makes it the only name a detected-only
/// agent gives its work.
pub fn screen_title(title: &str, label: &str, cwd: Option<&str>) -> Option<String> {
    task_title(title, label).filter(|title| !is_cwd_echo(title, cwd))
}

/// Drop the status glyph agents prefix their title with (`✳`, `⏺`, `●`). The row already has a
/// glyph column, so a second one beside the text is noise. Only non-ASCII symbols go — ASCII
/// punctuation is load-bearing in a real title (`~/repo`, `[2/7] running tests`).
pub fn strip_title_decoration(title: &str) -> &str {
    title
        .trim_start_matches(|ch: char| {
            ch.is_whitespace() || (!ch.is_ascii() && !ch.is_alphanumeric())
        })
        .trim()
}

/// OpenCode reserves the beginning of its terminal title for its own short label. Rows already name
/// the agent `OpenCode`, so keep only the useful title text.
pub fn strip_agent_title_prefix<'a>(title: &'a str, kind_label: &str) -> &'a str {
    if kind_label.eq_ignore_ascii_case("OpenCode") {
        title
            .strip_prefix("OC |")
            .map(str::trim_start)
            .unwrap_or(title)
    } else {
        title
    }
}

/// Whether a terminal title is just the working directory. A shell sets its title to `$PWD`, so an
/// agent that never set one of its own leaves a stale path there — and the project header already
/// says where the row is, so echoing it as the work spends the line on nothing.
fn is_cwd_echo(title: &str, cwd: Option<&str>) -> bool {
    let Some(cwd) = cwd else {
        return false;
    };
    if crate::platform::paths::paths_equal(title, cwd) {
        return true;
    }
    // `~/repo` against `/home/you/repo`: the last segment is what the two spellings share. A
    // one-word title that happens to match the directory name is lost to this, which is a fair
    // trade for never presenting a path as work.
    match (path_segments(title).last(), path_segments(cwd).last()) {
        (Some(title), Some(cwd)) => title.eq_ignore_ascii_case(cwd),
        _ => false,
    }
}
