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
    /// Reversible representation used for persistent identity and command-line arguments.
    ///
    /// Unlike [`Self::display_label`], this preserves the distinction between a bare OpenSSH
    /// alias and an explicit `ssh://` endpoint, and brackets IPv6 hosts so the result parses back
    /// into the same target.
    pub fn to_spec(&self) -> String {
        match self {
            Self::Alias(alias) => alias.clone(),
            Self::Url { user, host, port } => {
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

    /// The login this target names outright. `None` means "whatever OpenSSH decides", which is a
    /// real answer, not a missing one: an ssh_config `User` or the local account may supply it.
    pub fn user(&self) -> Option<&str> {
        match self {
            Self::Alias(_) => None,
            Self::Url { user, .. } => user.as_deref(),
        }
    }

    /// The machine half on its own: the alias for an alias, the host for an endpoint.
    pub fn host(&self) -> &str {
        match self {
            Self::Alias(alias) => alias,
            Self::Url { host, .. } => host,
        }
    }

    pub fn port(&self) -> Option<u16> {
        match self {
            Self::Alias(_) => None,
            Self::Url { port, .. } => *port,
        }
    }

    /// Rebuild a target from the fields of the host editor.
    ///
    /// A bare host with neither a login nor a port stays an [`Alias`](Self::Alias) so ssh_config
    /// keeps deciding everything about it; naming either one makes it an explicit endpoint, because
    /// that is the user overriding what ssh_config would have chosen.
    pub fn from_parts(host: &str, user: Option<&str>, port: Option<u16>) -> Result<Self, String> {
        let host = host.trim();
        let user = user.map(str::trim).filter(|user| !user.is_empty());
        if user.is_none() && port.is_none() {
            return parse_remote_target(host);
        }
        let target = Self::Url {
            user: user.map(str::to_string),
            host: host.to_string(),
            port,
        };
        validate_remote_target(&target)
            .map(|()| target)
            .map_err(|_| format!("invalid remote host `{host}`"))
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
        return parse_ssh_url(rest, raw);
    }
    if raw.contains("://") {
        return Err(format!(
            "unsupported remote URL scheme in `{raw}` (only ssh:// is accepted)"
        ));
    }
    if raw.contains('/') || raw.contains(' ') {
        return Err(format!("invalid remote target `{raw}`"));
    }
    // `adam@workbox` and `workbox:2222` are how people write an SSH endpoint by hand, so they parse
    // as one rather than as an alias that happens to contain punctuation. Keeping the login in the
    // target — instead of burying it in an opaque alias string — is what lets the host editor show
    // it, and what lets `adam@box` and `box` stay two rows rather than colliding on one label.
    if looks_like_endpoint(raw) {
        return parse_ssh_url(raw, raw);
    }
    Ok(RemoteTarget::Alias(raw.to_string()))
}

/// Whether bare input spells out a login or a port rather than naming an ssh_config alias.
fn looks_like_endpoint(raw: &str) -> bool {
    raw.contains('@')
        || raw.starts_with('[')
        || raw.rsplit_once(':').is_some_and(|(host, port)| {
            !host.is_empty()
                && !host.contains(':')
                && !port.is_empty()
                && port.chars().all(|ch| ch.is_ascii_digit())
        })
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

fn parse_ssh_url(rest: &str, shown: &str) -> Result<RemoteTarget, String> {
    if rest.is_empty() {
        return Err(format!("`{shown}` is missing a host"));
    }
    let (user, hostport) = match rest.split_once('@') {
        Some((user, hostport)) => {
            if user.is_empty() || user.contains('/') || user.contains(' ') {
                return Err(format!("invalid user in remote target `{shown}`"));
            }
            (Some(user.to_string()), hostport)
        }
        None => (None, rest),
    };
    if hostport.is_empty() {
        return Err(format!("`{shown}` is missing a host"));
    }
    // Bracketed IPv6: ssh://[::1]:2222 or ssh://user@[::1]
    let (host, port) = if let Some(inner) = hostport.strip_prefix('[') {
        let Some((host, after)) = inner.split_once(']') else {
            return Err(format!("invalid IPv6 host in remote target `{shown}`"));
        };
        if host.is_empty() {
            return Err(format!("`{shown}` is missing a host"));
        }
        let port = match after.strip_prefix(':') {
            Some(p) if !p.is_empty() => Some(parse_port(p)?),
            Some(_) => return Err(format!("`{shown}` has an empty port")),
            None if after.is_empty() => None,
            None => {
                return Err(format!("invalid remote target `{shown}`"));
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
            return Err(format!("`{shown}` is missing a host"));
        }
        (host.to_string(), Some(parse_port(port)?))
    } else {
        if hostport.contains('/') || hostport.contains(' ') {
            return Err(format!("invalid host in remote target `{shown}`"));
        }
        (hostport.to_string(), None)
    };
    Ok(RemoteTarget::Url { user, host, port })
}

fn parse_port(raw: &str) -> Result<u16, String> {
    raw.parse::<u16>()
        .map_err(|_| format!("invalid port `{raw}`"))
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

    /// What a user types into *Add host* is SSH's own shorthand, not an alias with punctuation in
    /// it. The login has to survive into the target or the host editor has nothing to show and
    /// nothing to correct.
    #[test]
    fn bare_endpoints_keep_their_login_and_port() {
        assert_eq!(
            parse_remote_target("adam@10.0.0.5").unwrap(),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "10.0.0.5".into(),
                port: None,
            }
        );
        assert_eq!(
            parse_remote_target("adam@workbox:2222").unwrap(),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "workbox".into(),
                port: Some(2222),
            }
        );
        assert_eq!(
            parse_remote_target("workbox:2222").unwrap(),
            RemoteTarget::Url {
                user: None,
                host: "workbox".into(),
                port: Some(2222),
            }
        );
        // A plain name is still ssh_config's to resolve, and stays an alias.
        assert_eq!(
            parse_remote_target("workbox").unwrap(),
            RemoteTarget::Alias("workbox".into())
        );
    }

    #[test]
    fn parts_become_an_alias_only_when_nothing_is_overridden() {
        assert_eq!(
            RemoteTarget::from_parts("workbox", None, None).unwrap(),
            RemoteTarget::Alias("workbox".into())
        );
        assert_eq!(
            RemoteTarget::from_parts("workbox", Some("  "), None).unwrap(),
            RemoteTarget::Alias("workbox".into()),
            "an emptied login field is `let ssh decide`, not an empty user"
        );
        assert_eq!(
            RemoteTarget::from_parts("workbox", Some("adam"), Some(2222)).unwrap(),
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "workbox".into(),
                port: Some(2222),
            }
        );
        assert!(RemoteTarget::from_parts("", Some("adam"), None).is_err());
    }

    #[test]
    fn target_parts_read_back_out() {
        let target = parse_remote_target("adam@workbox:2222").unwrap();
        assert_eq!(target.user(), Some("adam"));
        assert_eq!(target.host(), "workbox");
        assert_eq!(target.port(), Some(2222));

        let alias = parse_remote_target("workbox").unwrap();
        assert_eq!(alias.user(), None);
        assert_eq!(alias.host(), "workbox");
        assert_eq!(alias.port(), None);
    }

    #[test]
    fn canonical_specs_round_trip() {
        for target in [
            RemoteTarget::Alias("workbox".into()),
            RemoteTarget::Url {
                user: None,
                host: "example.com".into(),
                port: None,
            },
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "example.com".into(),
                port: Some(2222),
            },
            RemoteTarget::Url {
                user: Some("adam".into()),
                host: "2001:db8::1".into(),
                port: Some(2222),
            },
        ] {
            assert_eq!(parse_remote_target(&target.to_spec()).unwrap(), target);
        }
    }

    #[test]
    fn alias_and_url_with_the_same_label_keep_distinct_specs() {
        let alias = RemoteTarget::Alias("box".into());
        let url = RemoteTarget::Url {
            user: None,
            host: "box".into(),
            port: None,
        };
        assert_eq!(alias.display_label(), url.display_label());
        assert_ne!(alias, url);
        assert_eq!(alias.to_spec(), "box");
        assert_eq!(url.to_spec(), "ssh://box");
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
            "rozi",
            "/usr/local/bin/rozi",
            "C:/Users/me/rozi.exe",
            r"C:\Users\me\rozi.exe",
        ] {
            validate_remote_executable_token(token).expect(token);
        }
        for token in [
            "ro zi",
            "rozi\t--help",
            "rozi\n--help",
            "rozi;touch /tmp/pwned",
            "rozi$(id)",
            "rozi`id`",
            "rozi|cat",
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
