use tui_lipan::prelude::{Context, Element, Text};

use crate::AppRoot;
use crate::session::remote::{RemoteTarget, parse_remote_target};
use crate::state::{Attachment, is_ephemeral_session_name};

/// Build the one-shot view rendered after the client leaves the event loop.
pub(crate) fn exit_view(_component: &AppRoot, ctx: &Context<AppRoot>) -> Element {
    exit_summary(ctx.state.current())
        .map_or_else(Element::default, |summary| Text::new(summary).into())
}

/// Return a reattach hint for the foreground named session, if there is one.
pub(crate) fn exit_summary(attachment: &Attachment) -> Option<String> {
    let name = attachment.session_name.as_deref()?;
    if is_ephemeral_session_name(name)
        || contains_terminal_control(name)
        || !is_portable_shell_word(name)
        || remote_state_contains_control(attachment)
    {
        return None;
    }

    let has_remote_state = attachment.remote_host.is_some() || attachment.remote_target.is_some();
    if !has_remote_state {
        return Some(format!(
            "Detached from {name}\nReattach: hyprmux attach {name}"
        ));
    }

    let remote_target = remote_target_argument(attachment)?;
    if !is_portable_shell_word(&remote_target)
        || attachment
            .remote_target
            .as_ref()
            .is_some_and(|target| !is_portable_shell_word(&remote_target_to_argument(target)))
    {
        return None;
    }
    let identity = remote_identity(attachment, &remote_target);
    Some(format!(
        "Detached from {name}@{identity}\nReattach: hyprmux --remote {remote_target} attach {name}"
    ))
}

fn contains_terminal_control(value: &str) -> bool {
    value.chars().any(|character| character.is_control())
}

fn remote_state_contains_control(attachment: &Attachment) -> bool {
    attachment
        .remote_host
        .as_deref()
        .is_some_and(contains_terminal_control)
        || attachment
            .remote_target
            .as_ref()
            .is_some_and(remote_target_contains_control)
}

fn remote_target_contains_control(target: &RemoteTarget) -> bool {
    match target {
        RemoteTarget::Alias(alias) => contains_terminal_control(alias),
        RemoteTarget::Url { user, host, .. } => {
            user.as_deref().is_some_and(contains_terminal_control)
                || contains_terminal_control(host)
        }
    }
}

fn remote_target_argument(attachment: &Attachment) -> Option<String> {
    let raw_host = attachment
        .remote_host
        .as_deref()
        .filter(|host| !host.is_empty());

    match (raw_host, attachment.remote_target.as_ref()) {
        // A URL target's display label omits the `ssh://` marker. Keep the exact target when the
        // stored host is that label; otherwise prefer the stored raw host spelling.
        (Some(host), Some(target))
            if host == target.display_label() && matches!(target, RemoteTarget::Url { .. }) =>
        {
            Some(remote_target_to_argument(target))
        }
        (Some(host), _) => Some(host.to_string()),
        (None, Some(target)) => Some(remote_target_to_argument(target)),
        (None, None) => None,
    }
}

fn remote_identity(attachment: &Attachment, target: &str) -> String {
    attachment
        .remote_target
        .as_ref()
        .map(RemoteTarget::display_label)
        .or_else(|| {
            parse_remote_target(target)
                .ok()
                .map(|target| target.display_label())
        })
        .unwrap_or_else(|| target.strip_prefix("ssh://").unwrap_or(target).to_string())
}

fn remote_target_to_argument(target: &RemoteTarget) -> String {
    match target {
        RemoteTarget::Alias(alias) => alias.clone(),
        RemoteTarget::Url { user, host, port } => {
            let user = user
                .as_deref()
                .map_or(String::new(), |user| format!("{user}@"));
            let host = if host.contains(':') {
                format!("[{host}]")
            } else {
                host.clone()
            };
            let port = port.map_or(String::new(), |port| format!(":{port}"));
            format!("ssh://{user}{host}{port}")
        }
    }
}

/// Restrict copied arguments to syntax that remains one literal argument in POSIX shells, cmd, and
/// PowerShell. Values outside this set are omitted rather than rendered with shell-specific quoting.
fn is_portable_shell_word(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| {
            matches!(
                byte,
                b'a'..=b'z'
                    | b'A'..=b'Z'
                    | b'0'..=b'9'
                    | b'_' | b'-' | b'.' | b'/' | b':' | b'@'
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(name: Option<&str>) -> Attachment {
        let mut attachment = Attachment::new();
        attachment.session_name = name.map(str::to_owned);
        attachment
    }

    #[test]
    fn local_named_session_has_reattach_hint() {
        let attachment = attachment(Some("dev"));

        assert_eq!(
            exit_summary(&attachment).as_deref(),
            Some("Detached from dev\nReattach: hyprmux attach dev")
        );
    }

    #[test]
    fn remote_named_session_uses_raw_host_and_attach_only() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("workbox".to_string());

        assert_eq!(
            exit_summary(&attachment).as_deref(),
            Some("Detached from dev@workbox\nReattach: hyprmux --remote workbox attach dev")
        );
    }

    #[test]
    fn hostile_remote_alias_suppresses_summary() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("workbox;id".to_string());

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn embedded_single_quote_suppresses_summary() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("workbox'quoted".to_string());

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn remote_ssh_url_preserves_user_and_port() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("ssh://alice@example.com:2222".to_string());

        assert_eq!(
            exit_summary(&attachment).as_deref(),
            Some(
                "Detached from dev@alice@example.com:2222\nReattach: hyprmux --remote ssh://alice@example.com:2222 attach dev"
            )
        );
    }

    #[test]
    fn remote_ipv6_target_is_suppressed_without_portable_quoting() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("alice@::1:2222".to_string());
        attachment.remote_target = Some(RemoteTarget::Url {
            user: Some("alice".to_string()),
            host: "::1".to_string(),
            port: Some(2222),
        });

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn remote_control_character_suppresses_summary() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_host = Some("workbox\u{1b}[31m".to_string());

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn remote_target_control_character_suppresses_summary() {
        let mut attachment = attachment(Some("dev"));
        attachment.remote_target = Some(RemoteTarget::Alias("workbox\nnext".to_string()));

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn ephemeral_session_has_no_exit_summary() {
        let attachment = attachment(Some("eph-1234"));

        assert_eq!(exit_summary(&attachment), None);
    }

    #[test]
    fn sessionless_attachment_has_no_exit_summary() {
        let attachment = attachment(None);

        assert_eq!(exit_summary(&attachment), None);
    }
}
