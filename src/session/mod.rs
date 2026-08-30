pub(crate) mod bootstrap;
pub mod client;
pub mod discovery;
pub mod protocol;
pub(crate) mod queue;
pub mod remote;
pub mod server;

/// The map key for the local machine's own last session, alongside one key per remote target spec.
/// `local` is not a valid [`remote::RemoteTarget::to_spec`] output (those are `ssh://…` or a bare
/// alias, and an alias may not contain a scheme), so it cannot collide with a host.
const LOCAL_SCOPE_KEY: &str = "local";

/// Which workplace a "last session" memory belongs to: this machine, or one exact remote host.
///
/// `startup = "last"` reopens the last session *of the scope it launches into*. A bare `rozi` must
/// not reach for a name that only ever existed on `workbox`, and `rozi --remote workbox` must not
/// reopen the local one — they are different workplaces that happen to share one setting. Keying on
/// the canonical target spec keeps `dev@box:22` and `box` apart the same way the host registry does.
fn last_session_scope_key(scope: Option<&remote::RemoteTarget>) -> String {
    match scope {
        None => LOCAL_SCOPE_KEY.to_string(),
        Some(target) => target.to_spec(),
    }
}

fn last_sessions_path() -> Option<std::path::PathBuf> {
    let env = crate::platform::paths::PlatformEnv::from_process();
    (env.home.is_some() || env.xdg_state_home.is_some())
        .then(|| crate::platform::paths::state_dir(&env).join("last-sessions.json"))
}

fn read_last_sessions() -> std::collections::HashMap<String, String> {
    let Some(path) = last_sessions_path() else {
        return std::collections::HashMap::new();
    };
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_last_sessions(entries: &std::collections::HashMap<String, String>) {
    let Some(path) = last_sessions_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(entries) {
        let _ = std::fs::write(path, text);
    }
}

pub(crate) fn record_last_session(scope: Option<&remote::RemoteTarget>, name: &str) {
    if !discovery::valid_session_name(name) {
        return;
    }
    let mut entries = read_last_sessions();
    entries.insert(last_session_scope_key(scope), name.to_string());
    write_last_sessions(&entries);
}

pub(crate) fn read_last_session(scope: Option<&remote::RemoteTarget>) -> Option<String> {
    let entries = read_last_sessions();
    let name = entries.get(&last_session_scope_key(scope))?;
    discovery::valid_session_name(name).then(|| name.clone())
}

/// Drop one host's last-session memory, for the same reason forgetting a host drops its session
/// cache: the user asked for that machine to stop being one of their workplaces.
pub(crate) fn forget_last_session(scope: Option<&remote::RemoteTarget>) {
    let mut entries = read_last_sessions();
    if entries.remove(&last_session_scope_key(scope)).is_some() {
        write_last_sessions(&entries);
    }
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

fn update_recent_targets(entries: &mut Vec<remote::RemoteTarget>, target: &remote::RemoteTarget) {
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

/// Per-host cache of last-seen sessions, keyed by [`remote::RemoteTarget::to_spec`]. Persisted
/// under the state dir. Readers accept legacy display-label keys until the next mutation.
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

/// Read one target's cache by canonical identity, with a legacy display-label fallback.
pub(crate) fn host_sessions_for<'a>(
    cache: &'a HostSessionCache,
    target: &remote::RemoteTarget,
) -> Option<&'a [CachedHostSession]> {
    cache
        .get(&target.to_spec())
        .or_else(|| cache.get(&target.display_label()))
        .map(Vec::as_slice)
}

pub(crate) fn host_cache_contains_target(
    cache: &HostSessionCache,
    target: &remote::RemoteTarget,
) -> bool {
    host_sessions_for(cache, target).is_some()
}

/// Install a canonical in-memory entry. A different legacy display-label key is retained because
/// it may now be the canonical key of an alias with the same label; deleting it would collapse the
/// exact identity this migration is meant to preserve.
pub(crate) fn set_cached_host_sessions(
    cache: &mut HostSessionCache,
    target: &remote::RemoteTarget,
    sessions: Vec<CachedHostSession>,
) {
    let canonical = target.to_spec();
    cache.insert(canonical, sessions);
}

pub(crate) fn remove_cached_host_sessions(
    cache: &mut HostSessionCache,
    target: &remote::RemoteTarget,
) {
    let canonical = target.to_spec();
    let legacy = target.display_label();
    let had_canonical = cache.remove(&canonical).is_some();
    if legacy != canonical && !had_canonical {
        cache.remove(&legacy);
    }
}

fn write_host_session_cache(cache: &HostSessionCache) {
    let Some(path) = host_sessions_path() else {
        return;
    };
    if let Some(parent) = path.parent()
        && crate::platform::fs_security::ensure_private_dir(parent).is_err()
    {
        return;
    }
    if let Ok(text) = serde_json::to_string_pretty(cache) {
        let _ = std::fs::write(path, text);
    }
}

/// Replace the cached session list for one host after a successful probe. An empty list is retained
/// (the host answered with no sessions) so a stale non-empty entry does not linger. Writing the
/// whole map keeps the file self-consistent.
pub(crate) fn record_host_sessions(
    target: &remote::RemoteTarget,
    sessions: Vec<CachedHostSession>,
) {
    let mut cache = read_host_session_cache();
    set_cached_host_sessions(&mut cache, target, sessions);
    write_host_session_cache(&cache);
}

/// Remove both canonical and legacy cache identities for one exact target.
pub(crate) fn forget_host_sessions(target: &remote::RemoteTarget) {
    let mut cache = read_host_session_cache();
    remove_cached_host_sessions(&mut cache, target);
    write_host_session_cache(&cache);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `startup = "last"` means "last here", and `here` is the machine the launch names. A local
    /// launch reaching for a session that only ever existed on `workbox` would try to start a
    /// same-named local server; the reverse would strand a `--remote workbox` launch on a name that
    /// host has never heard of.
    #[test]
    fn last_session_is_remembered_per_workplace() {
        crate::test_support::isolate_user_dirs();
        let workbox = remote::RemoteTarget::Alias("workbox".into());
        let other = remote::RemoteTarget::Url {
            user: Some("dev".into()),
            host: "workbox".into(),
            port: Some(22),
        };

        record_last_session(None, "local-dev");
        record_last_session(Some(&workbox), "backend");

        assert_eq!(read_last_session(None).as_deref(), Some("local-dev"));
        assert_eq!(
            read_last_session(Some(&workbox)).as_deref(),
            Some("backend")
        );
        // Same display label, different SSH endpoint: a different workplace, as everywhere else.
        assert_eq!(read_last_session(Some(&other)), None);

        // Writing one scope leaves the others alone.
        record_last_session(Some(&workbox), "api");
        assert_eq!(read_last_session(Some(&workbox)).as_deref(), Some("api"));
        assert_eq!(read_last_session(None).as_deref(), Some("local-dev"));

        forget_last_session(Some(&workbox));
        assert_eq!(read_last_session(Some(&workbox)), None);
        assert_eq!(read_last_session(None).as_deref(), Some("local-dev"));
    }

    /// An unusable name never reaches the file, so a later launch cannot be handed one to attach.
    #[test]
    fn an_invalid_session_name_is_not_remembered() {
        crate::test_support::isolate_user_dirs();
        let target = remote::RemoteTarget::Alias("scratchbox".into());
        record_last_session(Some(&target), "not a session name");
        assert_eq!(read_last_session(Some(&target)), None);
    }

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
    fn host_cache_prefers_canonical_identity_and_migrates_legacy_on_write() {
        let alias = remote::RemoteTarget::Alias("box".into());
        let url = remote::RemoteTarget::Url {
            user: None,
            host: "box".into(),
            port: None,
        };
        let legacy = vec![CachedHostSession {
            name: "legacy".into(),
            ephemeral: false,
            panes: 1,
        }];
        let canonical = vec![CachedHostSession {
            name: "canonical".into(),
            ephemeral: false,
            panes: 2,
        }];
        let mut cache = HostSessionCache::new();
        cache.insert("box".into(), legacy.clone());
        cache.insert("ssh://box".into(), canonical.clone());

        assert_eq!(host_sessions_for(&cache, &alias), Some(legacy.as_slice()));
        assert_eq!(host_sessions_for(&cache, &url), Some(canonical.as_slice()));

        set_cached_host_sessions(&mut cache, &url, legacy.clone());
        assert_eq!(cache.get("ssh://box"), Some(&legacy));
        assert_eq!(cache.get("box"), Some(&legacy));

        remove_cached_host_sessions(&mut cache, &url);
        assert!(!cache.contains_key("ssh://box"));
        assert_eq!(cache.get("box"), Some(&legacy));
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
