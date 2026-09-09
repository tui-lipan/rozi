//! One read-only executable resolver for discovery, monitoring, and session commands.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{RemoteTarget, ResolvedRemote, bootstrap, validate_remote_executable_token};
use crate::config::RemoteConfig;

struct CachedBinary {
    remote: ResolvedRemote,
    path: String,
    checked: Instant,
}

// Short-lived hints, never persisted. Failed commands invalidate them, and changed SSH settings
// select a different entry. Bounding the roster avoids retaining every host visited forever.
static CACHE: Mutex<Vec<CachedBinary>> = Mutex::new(Vec::new());
const MAX_AGE: Duration = Duration::from_secs(60);

pub(super) fn cached(target: &RemoteTarget, config: &RemoteConfig) -> Option<String> {
    let remote = ResolvedRemote::resolve(target, config);
    CACHE
        .lock()
        .ok()?
        .iter()
        .find(|entry| entry.remote == remote && entry.checked.elapsed() < MAX_AGE)
        .map(|entry| entry.path.clone())
}

pub(super) fn remember(
    target: &RemoteTarget,
    config: &RemoteConfig,
    path: String,
) -> Result<String, String> {
    validate_remote_executable_token(&path)?;
    let remote = ResolvedRemote::resolve(target, config);
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|entry| entry.remote != remote && entry.checked.elapsed() < MAX_AGE);
        if cache.len() >= 128 {
            cache.remove(0);
        }
        cache.push(CachedBinary {
            remote,
            path: path.clone(),
            checked: Instant::now(),
        });
    }
    Ok(path)
}

pub(crate) fn invalidate(target: &RemoteTarget, config: &RemoteConfig) {
    let remote = ResolvedRemote::resolve(target, config);
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|entry| entry.remote != remote);
    }
}

/// Find a compatible executable without ever installing or honoring an upload override.
pub(crate) fn resolve(target: &RemoteTarget, config: &RemoteConfig) -> Result<String, String> {
    super::validate_remote_target(target)?;
    if let Some(path) = cached(target, config) {
        return Ok(path);
    }
    match bootstrap::probe_remote(target, config)? {
        bootstrap::ProbeResult::Found { path, .. } => remember(target, config, path),
        bootstrap::ProbeResult::Missing { detail } => Err(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_paths_follow_connection_settings_and_invalidation() {
        let target = RemoteTarget::Alias("binary-cache-fixture".into());
        let mut config = RemoteConfig::default();
        remember(&target, &config, "/home/u/.local/bin/rozi".into()).unwrap();
        assert_eq!(
            cached(&target, &config).as_deref(),
            Some("/home/u/.local/bin/rozi")
        );
        config.hosts.insert(
            "binary-cache-fixture".into(),
            crate::config::RemoteHostConfig {
                user: Some("another-user".into()),
                ..Default::default()
            },
        );
        assert!(cached(&target, &config).is_none());
        config.hosts.clear();
        invalidate(&target, &config);
        assert!(cached(&target, &config).is_none());
        assert!(remember(&target, &config, "rozi;touch /tmp/no".into()).is_err());
        assert!(cached(&target, &config).is_none());
    }
}
