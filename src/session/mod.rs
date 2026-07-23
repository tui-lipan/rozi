pub(crate) mod bootstrap;
pub mod client;
pub mod discovery;
pub mod protocol;
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
