pub(crate) mod bootstrap;
pub mod client;
pub mod discovery;
pub mod protocol;
pub(crate) mod queue;
pub mod remote;
pub mod server;

fn last_session_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("last-session"))
}

pub(crate) fn record_last_named_session(name: &str) {
    if !discovery::valid_session_name(name) {
        return;
    }
    let Some(path) = last_session_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    let _ = std::fs::write(path, format!("{name}\n"));
}

pub(crate) fn read_last_named_session() -> Option<String> {
    let name = std::fs::read_to_string(last_session_path()?).ok()?;
    let name = name.trim();
    discovery::valid_session_name(name).then(|| name.to_string())
}

/// The most recently used remote targets, most-recent first. Only canonical target specs are
/// stored — never a password or key, which SSH handles out of band.
const MAX_RECENT_REMOTES: usize = 10;

fn recent_remotes_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("recent-remotes"))
}

fn write_recent_remotes(entries: &[remote::RemoteTarget]) {
    let Some(path) = recent_remotes_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    let text = entries
        .iter()
        .map(remote::RemoteTarget::to_spec)
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(path, text);
}

fn update_recent_targets(
    entries: &mut Vec<remote::RemoteTarget>,
    target: &remote::RemoteTarget,
) {
    entries.retain(|entry| entry != target);
    entries.insert(0, target.clone());
    entries.truncate(MAX_RECENT_REMOTES);
}

/// Record a successfully-used remote target, moving it to the front and capping the list.
pub(crate) fn record_recent_remote(target: &remote::RemoteTarget) {
    let mut entries = read_recent_remotes();
    update_recent_targets(&mut entries, target);
    write_recent_remotes(&entries);
}

/// Forget one exact remote identity without affecting a target with the same display label.
pub(crate) fn forget_recent_remote(target: &remote::RemoteTarget) {
    let mut entries = read_recent_remotes();
    entries.retain(|entry| entry != target);
    write_recent_remotes(&entries);
}

/// Recently used ad-hoc remote targets, most-recent first.
pub(crate) fn read_recent_remotes() -> Vec<remote::RemoteTarget> {
    let Some(path) = recent_remotes_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .map(|text| {
            let mut entries = Vec::new();
            for target in text
                .lines()
                .filter_map(|line| remote::parse_remote_target(line.trim()).ok())
            {
                if !entries.contains(&target) {
                    entries.push(target);
                }
                if entries.len() == MAX_RECENT_REMOTES {
                    break;
                }
            }
            entries
        })
        .unwrap_or_default()
}

/// A last-seen session on a remote host, cached so a host's known workplaces stay visible when it is
/// offline. Only the display metadata is stored — name, whether it was ephemeral, and pane count —
/// never any credential or key material, which SSH handles out of band.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CachedHostSession {
    pub name: String,
    #[serde(default)]
    pub ephemeral: bool,
    #[serde(default)]
    pub panes: usize,
}

/// Per-host cache of last-seen sessions, keyed by the host's exact display target (so `box` and
/// `dev@box:22` cache separately). Persisted under the state dir.
pub type HostSessionCache = std::collections::HashMap<String, Vec<CachedHostSession>>;

fn host_sessions_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("host-sessions.json"))
}

/// Read the persisted per-host session cache. Empty on any error (missing file, parse failure): the
/// cache is a convenience, never a source of truth.
pub(crate) fn read_host_session_cache() -> HostSessionCache {
    let Some(path) = host_sessions_path() else {
        return HostSessionCache::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Replace the cached session list for one host after a successful probe. An empty list is retained
/// (the host answered with no sessions) so a stale non-empty entry does not linger. Writing the
/// whole map keeps the file self-consistent.
pub(crate) fn record_host_sessions(target_label: &str, sessions: Vec<CachedHostSession>) {
    let target_label = target_label.trim();
    if target_label.is_empty() {
        return;
    }
    let Some(path) = host_sessions_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    let mut cache = read_host_session_cache();
    cache.insert(target_label.to_string(), sessions);
    if let Ok(text) = serde_json::to_string_pretty(&cache) {
        let _ = std::fs::write(path, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The host session cache is a plain string-keyed map of session summaries; it must survive a
    /// serialize/parse round-trip so an offline host's workplaces reload intact, and it must hold no
    /// field beyond name/ephemeral/panes (never a credential).
    #[test]
    fn host_session_cache_round_trips() {
        let sessions = vec![
            CachedHostSession {
                name: "dev".into(),
                ephemeral: false,
                panes: 3,
            },
            CachedHostSession {
                name: "api".into(),
                ephemeral: false,
                panes: 1,
            },
        ];
        let mut cache = HostSessionCache::new();
        cache.insert("workbox".into(), sessions.clone());
        let json = serde_json::to_string(&cache).unwrap();
        let back: HostSessionCache = serde_json::from_str(&json).unwrap();
        assert_eq!(back.get("workbox"), Some(&sessions));
        // Missing optional fields default rather than failing to parse.
        let sparse: HostSessionCache =
            serde_json::from_str(r#"{"box":[{"name":"only"}]}"#).unwrap();
        assert_eq!(sparse["box"][0].name, "only");
        assert_eq!(sparse["box"][0].panes, 0);
    }

    #[test]
    fn typed_recents_deduplicate_move_to_front_and_cap() {
        let mut entries = (0..MAX_RECENT_REMOTES)
            .map(|index| remote::RemoteTarget::Alias(format!("host-{index}")))
            .collect::<Vec<_>>();
        let reused = entries[5].clone();
        update_recent_targets(&mut entries, &reused);
        assert_eq!(entries.first(), Some(&reused));
        assert_eq!(entries.len(), MAX_RECENT_REMOTES);
        assert_eq!(entries.iter().filter(|entry| *entry == &reused).count(), 1);

        update_recent_targets(
            &mut entries,
            &remote::RemoteTarget::Url {
                user: Some("adam".into()),
                host: "new.example".into(),
                port: Some(2222),
            },
        );
        assert_eq!(entries.len(), MAX_RECENT_REMOTES);
        assert_eq!(
            entries.first().unwrap().to_spec(),
            "ssh://adam@new.example:2222"
        );
    }

    #[test]
    fn recent_lines_parse_typed_identity_and_ignore_malformed_values() {
        let parsed = ["workbox", "ssh://workbox", "bad target"]
            .into_iter()
            .filter_map(|line| remote::parse_remote_target(line).ok())
            .collect::<Vec<_>>();
        assert_eq!(parsed.len(), 2);
        assert_ne!(parsed[0], parsed[1]);
        assert_eq!(parsed[0].to_spec(), "workbox");
        assert_eq!(parsed[1].to_spec(), "ssh://workbox");
    }
}
