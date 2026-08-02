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

/// The most recently used ad-hoc `--remote` targets, most-recent first. Persisted so the
/// "Connect remote host…" prompt can offer them without re-typing. Only the target string is stored
/// (host / user@host:port / ssh:// URL) - never a password or key, which SSH handles out of band.
const MAX_RECENT_REMOTES: usize = 10;

fn recent_remotes_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("recent-remotes"))
}

/// Record a successfully-used ad-hoc remote target, moving it to the front and capping the list.
pub(crate) fn record_recent_remote(target: &str) {
    let target = target.trim();
    if target.is_empty() {
        return;
    }
    let Some(path) = recent_remotes_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    let mut entries = read_recent_remotes();
    entries.retain(|entry| entry != target);
    entries.insert(0, target.to_string());
    entries.truncate(MAX_RECENT_REMOTES);
    let _ = std::fs::write(path, entries.join("\n"));
}

/// Recently used ad-hoc remote targets, most-recent first.
pub(crate) fn read_recent_remotes() -> Vec<String> {
    let Some(path) = recent_remotes_path() else {
        return Vec::new();
    };
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string)
                .collect()
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
}
