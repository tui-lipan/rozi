//! Cross-platform desktop notifications (cross-platform plan Phase 10).
//!
//! One user-visible behavior - "a pane exited" ([`crate::pty_events::maybe_notify_pane_exit`],
//! gated on `[notifications] enabled` / `pane_exit`) - reached through whatever the host actually
//! has:
//!
//! | Platform | Mechanism |
//! |---|---|
//! | Linux/BSD | `notify-send` (the freedesktop `org.freedesktop.Notifications` CLI) |
//! | macOS | `osascript -e 'display notification ...'` |
//! | Windows | PowerShell + the WinRT toast API |
//!
//! Best-effort throughout: a host without the relevant tool simply shows no notification. That is
//! deliberately silent rather than a toast saying "could not toast" - the user asked for a desktop
//! notification, not for rozi to talk about itself - and it is why nothing here returns a result.
//!
//! Every one of these launches an external program, so the notification body is data, never code:
//! it is passed as a distinct `Command::arg` on Unix and, on Windows (where the body has to cross
//! into a PowerShell script), through an environment variable the script reads back rather than
//! through string interpolation. A pane title or command name containing a quote is inert either
//! way.

/// Show a desktop notification, off-thread. Silent on any failure, including a host with no
/// notification mechanism at all.
pub fn notify(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = show(&summary, &body);
    });
}

#[cfg(target_os = "macos")]
fn show(summary: &str, body: &str) -> std::io::Result<()> {
    // `display notification` takes AppleScript *string literals*, so a quote in either argument
    // would otherwise terminate the literal and let the rest be parsed as script. Escape both.
    let script = format!(
        "display notification {} with title {}",
        applescript_string(body),
        applescript_string(summary),
    );
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .status()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn applescript_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', r"\\").replace('"', "\\\""))
}

#[cfg(windows)]
fn show(summary: &str, body: &str) -> std::io::Result<()> {
    // The text is handed to PowerShell through the environment, never interpolated into the script
    // source - so no amount of quoting or backtick trickery in a pane's title can become script.
    const SCRIPT: &str = "\
        [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime] | Out-Null;\
        $template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02);\
        $nodes = $template.GetElementsByTagName('text');\
        $nodes.Item(0).AppendChild($template.CreateTextNode($env:ROZI_NOTIFY_SUMMARY)) | Out-Null;\
        $nodes.Item(1).AppendChild($template.CreateTextNode($env:ROZI_NOTIFY_BODY)) | Out-Null;\
        $toast = [Windows.UI.Notifications.ToastNotification]::new($template);\
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('rozi').Show($toast);";

    let program = super::command::lookup_program("powershell.exe")
        .or_else(|| super::command::lookup_program("pwsh.exe"))
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no PowerShell available for desktop notifications",
            )
        })?;

    let mut command = std::process::Command::new(program);
    command
        .arg("-NoProfile")
        .arg("-NonInteractive")
        .arg("-Command")
        .arg(SCRIPT)
        .env("ROZI_NOTIFY_SUMMARY", summary)
        .env("ROZI_NOTIFY_BODY", body)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    {
        use std::os::windows::process::CommandExt;
        use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.status().map(|_| ())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show(summary: &str, body: &str) -> std::io::Result<()> {
    std::process::Command::new("notify-send")
        .arg(summary)
        .arg(body)
        .status()
        .map(|_| ())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn applescript_strings_cannot_break_out_of_their_literal() {
        assert_eq!(
            applescript_string(r#"pane "sh" exited\"#),
            r#""pane \"sh\" exited\\""#
        );
    }
}
