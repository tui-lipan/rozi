//! Parse `--remote` target syntax: bare alias/host or `ssh://[user@]host[:port]`.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteTarget {
    /// ssh_config Host alias or bare hostname (may also match `[remote.hosts.<alias>]`).
    Alias(String),
    Url {
        user: Option<String>,
        host: String,
        port: Option<u16>,
    },
}

impl RemoteTarget {
    pub fn display_label(&self) -> String {
        match self {
            Self::Alias(alias) => alias.clone(),
            Self::Url { user, host, port } => {
                let user = user
                    .as_deref()
                    .map_or(String::new(), |user| format!("{user}@"));
                let port = port.map_or(String::new(), |port| format!(":{port}"));
                format!("{user}{host}{port}")
            }
        }
    }
}

/// Parse a `--remote` value. Rejects empty strings and malformed `ssh://` URLs.
pub fn parse_remote_target(raw: &str) -> Result<RemoteTarget, String> {
    if raw.chars().any(char::is_control) {
        return Err("invalid remote target: control characters are not allowed".to_string());
    }
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("--remote requires a host alias or ssh:// URL".to_string());
    }
    if let Some(rest) = raw.strip_prefix("ssh://") {
        return parse_ssh_url(rest);
    }
    if raw.contains("://") {
        return Err(format!(
            "unsupported remote URL scheme in `{raw}` (only ssh:// is accepted)"
        ));
    }
    if raw.contains('/') || raw.contains(' ') {
        return Err(format!("invalid remote target `{raw}`"));
    }
    Ok(RemoteTarget::Alias(raw.to_string()))
}

/// Validate a target assembled by a caller rather than parsed from the CLI. Target components are
/// passed to local `ssh` as arguments and may legitimately contain punctuation used by normal host
/// aliases, but control characters must never cross a remote-command boundary or reach a terminal.
pub(crate) fn validate_remote_target(target: &RemoteTarget) -> Result<(), String> {
    let invalid = |component: &str| component.is_empty() || component.chars().any(char::is_control);
    match target {
        RemoteTarget::Alias(alias) => {
            if invalid(alias) {
                return Err("invalid remote target: empty or control characters".to_string());
            }
        }
        RemoteTarget::Url { user, host, port } => {
            if user.as_deref().is_some_and(invalid) || invalid(host) || port == &Some(0) {
                return Err("invalid remote target: empty or control characters".to_string());
            }
        }
    }
    Ok(())
}

/// Validate a remote executable that OpenSSH will reconstruct into a remote shell command. Keep the
/// contract deliberately narrower than a host alias: only ordinary single-token path characters are
/// accepted, so whitespace, control bytes, quoting, expansion, globbing, and command separators can
/// never be reinterpreted by the remote shell.
pub(crate) fn validate_remote_executable_token(token: &str) -> Result<(), String> {
    if token.is_empty() {
        return Err("remote executable token is empty".to_string());
    }
    if token
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(
            "remote executable must be one shell-safe token without whitespace or control characters"
                .to_string(),
        );
    }
    if token.ends_with('\\')
        || !token.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(
                    ch,
                    '/' | '\\' | '.' | '_' | '-' | '+' | '=' | ':' | '@' | ','
                )
        })
    {
        return Err(
            "remote executable contains shell metacharacters; use a simple executable path"
                .to_string(),
        );
    }
    Ok(())
}

fn parse_ssh_url(rest: &str) -> Result<RemoteTarget, String> {
    if rest.is_empty() {
        return Err("ssh:// URL is missing a host".to_string());
    }
    let (user, hostport) = match rest.split_once('@') {
        Some((user, hostport)) => {
            if user.is_empty() || user.contains('/') || user.contains(' ') {
                return Err(format!("invalid user in ssh:// URL `ssh://{rest}`"));
            }
            (Some(user.to_string()), hostport)
        }
        None => (None, rest),
    };
    if hostport.is_empty() {
        return Err("ssh:// URL is missing a host".to_string());
    }
    // Bracketed IPv6: ssh://[::1]:2222 or ssh://user@[::1]
    let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
        let Some((host, after)) = inner.split_once(']') else {
            return Err(format!("invalid IPv6 host in ssh:// URL `ssh://{rest}`"));
        };
        if host.is_empty() {
            return Err("ssh:// URL is missing a host".to_string());
        }
        let port = match after.strip_prefix(':') {
            Some(p) if !p.is_empty() => Some(parse_port(p)?),
            Some(_) => return Err("ssh:// URL has an empty port".to_string()),
            None if after.is_empty() => None,
            None => {
                return Err(format!("invalid ssh:// URL `ssh://{rest}`"));
            }
        };
        (host.to_string(), port)
    } else if let Some((host, port)) = hostport.rsplit_once(':')
        && port.chars().all(|c| c.is_ascii_digit())
        && !port.is_empty()
        && !host.contains(':')
    {
        // Only treat as port when host has no other colons (not bare IPv6 without brackets).
        if host.is_empty() {
            return Err("ssh:// URL is missing a host".to_string());
        }
        (host.to_string(), Some(parse_port(port)?))
    } else {
        if hostport.contains('/') || hostport.contains(' ') {
            return Err(format!("invalid host in ssh:// URL `ssh://{rest}`"));
        }
        (hostport.to_string(), None)
    };
    Ok(RemoteTarget::Url { user, host, port })
}

fn parse_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .map_err(|_| format!("invalid port `{raw}` in ssh:// URL"))
        .and_then(|port| {
            if port == 0 {
                Err("port must be non-zero".to_string())
            } else {
                Ok(port)
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_alias() {
        assert_eq!(
            parse_remote_target("workbox").unwrap(),
            RemoteTarget::Alias("workbox".into())
        );
    }

    #[test]
    fn parses_ssh_url_variants() {
        assert_eq!(
            parse_remote_target("ssh://host").unwrap(),
            RemoteTarget::Url {
                user: None,
                host: "host".into(),
                port: None,
            }
        );
        assert_eq!(
            parse_remote_target("ssh://user@host:2222").unwrap(),
            RemoteTarget::Url {
                user: Some("user".into()),
                host: "host".into(),
                port: Some(2222),
            }
        );
        assert_eq!(
            parse_remote_target("ssh://[::1]:2222").unwrap(),
            RemoteTarget::Url {
                user: None,
                host: "::1".into(),
                port: Some(2222),
            }
        );
    }

    #[test]
    fn rejects_bad_targets() {
        assert!(parse_remote_target("").is_err());
        assert!(parse_remote_target("http://host").is_err());
        assert!(parse_remote_target("ssh://").is_err());
        assert!(parse_remote_target("ssh://host:0").is_err());
        assert!(parse_remote_target("work\u{1b}[31mbox").is_err());
        assert!(parse_remote_target("ssh://host\nnext").is_err());
    }

    #[test]
    fn validates_remote_executable_tokens_without_rejecting_normal_paths() {
        for token in [
            "hyprmux",
            "/usr/local/bin/hyprmux",
            "C:/Users/me/hyprmux.exe",
            r"C:\Users\me\hyprmux.exe",
        ] {
            validate_remote_executable_token(token).expect(token);
        }
        for token in [
            "hypr mux",
            "hyprmux\t--help",
            "hyprmux\n--help",
            "hyprmux;touch /tmp/pwned",
            "hyprmux$(id)",
            "hyprmux`id`",
            "hyprmux|cat",
            r"C:\Users\me\",
            "",
        ] {
            assert!(
                validate_remote_executable_token(token).is_err(),
                "accepted hostile executable token {token:?}"
            );
        }
    }

    #[test]
    fn display_label_keeps_user_and_port_identity() {
        assert_eq!(
            parse_remote_target("ssh://alice@example.com:2222")
                .unwrap()
                .display_label(),
            "alice@example.com:2222"
        );
        assert_eq!(
            parse_remote_target("workbox").unwrap().display_label(),
            "workbox"
        );
    }
}
