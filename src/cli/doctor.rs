//! A report safe to paste into a public bug report. Never print raw diagnostics or environment
//! values: config warnings and session replies can contain paths, commands, and credentials.

use super::output::print_or_stop;

pub(crate) fn run() {
    let loaded = crate::config::load_config();
    let mut fields = vec![("rozi".to_string(), env!("CARGO_PKG_VERSION").to_string())];
    for line in super::help::version_text().lines().skip(1) {
        if let Some((key, value)) = line.split_once('=') {
            fields.push((key.to_string(), value.to_string()));
        }
    }
    fields.extend([
        ("os".into(), std::env::consts::OS.into()),
        ("arch".into(), std::env::consts::ARCH.into()),
        (
            "install_source".into(),
            crate::platform::install_source::detect_current()
                .label()
                .into(),
        ),
        (
            "terminal".into(),
            terminal_name(
                std::env::var("TERM_PROGRAM").ok().as_deref(),
                std::env::var("TERM").ok().as_deref(),
            )
            .into(),
        ),
        (
            "config".into(),
            if loaded.rejected || !loaded.warnings.is_empty() {
                "invalid"
            } else {
                "valid"
            }
            .into(),
        ),
        ("config_errors".into(), loaded.warnings.len().to_string()),
        (
            "ssh_on_path".into(),
            if crate::platform::command::program_exists("ssh") {
                "yes"
            } else {
                "no"
            }
            .into(),
        ),
    ]);
    let sessions = crate::session::discovery::diagnostic_counts().ok();
    for (key, count) in [
        (
            "sessions_running",
            sessions.as_ref().map(|counts| counts.running),
        ),
        (
            "sessions_incompatible",
            sessions.as_ref().map(|counts| counts.incompatible),
        ),
        (
            "sessions_restorable",
            sessions.as_ref().map(|counts| counts.restorable),
        ),
        (
            "sessions_unknown",
            sessions.as_ref().map(|counts| counts.unknown),
        ),
    ] {
        fields.push((
            key.into(),
            count.map_or_else(|| "unavailable".into(), |count| count.to_string()),
        ));
    }
    let entries = crate::config::scan_extensions_for_cli().entries();
    fields.push(("extensions_installed".into(), entries.len().to_string()));
    // A loaded manifest does not mean its processes are running. Runtime status belongs to
    // individual clients and sessions; collecting it would require additional control requests.
    fields.push(("extensions_running".into(), "unavailable".into()));
    fields.push(("extensions_failed".into(), "unavailable".into()));
    let width = fields.iter().map(|(key, _)| key.len()).max().unwrap_or(0);
    let report: String = fields
        .into_iter()
        .map(|(key, value)| format!("{key:width$}  {value}\n"))
        .collect();
    print_or_stop(&report);
}

/// Recognize terminal names rather than echoing an arbitrary value supplied by the environment.
fn terminal_name(program: Option<&str>, term: Option<&str>) -> &'static str {
    const PROGRAMS: &[(&str, &str)] = &[
        ("ghostty", "ghostty"),
        ("wezterm", "wezterm"),
        ("iterm.app", "iTerm"),
        ("apple_terminal", "Apple Terminal"),
        ("vscode", "VS Code"),
        ("kitty", "kitty"),
        ("alacritty", "alacritty"),
        ("rozi", "rozi"),
        ("tmux", "tmux"),
    ];
    const TERMS: &[(&str, &str)] = &[
        ("xterm-kitty", "kitty"),
        ("alacritty", "alacritty"),
        ("foot", "foot"),
        ("foot-extra", "foot"),
        ("xterm-ghostty", "ghostty"),
        ("xterm", "xterm-compatible"),
        ("xterm-256color", "xterm-compatible"),
        ("xterm-direct", "xterm-compatible"),
        ("screen", "screen-compatible"),
        ("screen-256color", "screen-compatible"),
        ("tmux", "tmux"),
        ("tmux-256color", "tmux"),
        ("linux", "linux console"),
        ("dumb", "dumb"),
    ];
    let program = program.unwrap_or_default().to_ascii_lowercase();
    PROGRAMS
        .iter()
        .find(|(key, _)| *key == program)
        .or_else(|| TERMS.iter().find(|(key, _)| Some(*key) == term))
        .map_or("unknown", |(_, name)| *name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_environment_is_never_echoed() {
        assert_eq!(
            terminal_name(Some("WezTerm"), Some("xterm-256color")),
            "wezterm"
        );
        assert_eq!(terminal_name(None, Some("xterm-kitty")), "kitty");
        for private in [
            "/home/private/terminal",
            "user@host",
            "kitty\nsecret",
            "\x1b[31m",
        ] {
            assert_eq!(terminal_name(Some(private), Some(private)), "unknown");
        }
    }
}
