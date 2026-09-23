//! One read-only executable resolver for discovery, monitoring, and session commands.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{RemoteTarget, ResolvedRemote, bootstrap, validate_remote_executable_token};
use crate::config::RemoteConfig;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RemoteBinary {
    pub path: String,
    pub family: bootstrap::RemoteFamily,
}

struct CachedBinary {
    remote: ResolvedRemote,
    binary: RemoteBinary,
    checked: Instant,
}

// Short-lived hints, never persisted. Failed commands invalidate them, and changed SSH settings
// select a different entry. Bounding the roster avoids retaining every host visited forever.
static CACHE: Mutex<Vec<CachedBinary>> = Mutex::new(Vec::new());
const MAX_AGE: Duration = Duration::from_secs(60);

pub(super) fn cached(target: &RemoteTarget, config: &RemoteConfig) -> Option<RemoteBinary> {
    cached_matching(target, config, true)
}

/// Last remembered path for this destination, even after the 60s hint expires.
/// A dropped SSH link does not mean the remote executable moved.
pub(crate) fn last_known(target: &RemoteTarget, config: &RemoteConfig) -> Option<RemoteBinary> {
    cached_matching(target, config, false)
}

fn cached_matching(
    target: &RemoteTarget,
    config: &RemoteConfig,
    require_fresh: bool,
) -> Option<RemoteBinary> {
    let remote = ResolvedRemote::resolve(target, config);
    CACHE
        .lock()
        .ok()?
        .iter()
        .rev()
        .find(|entry| {
            entry.remote == remote && (!require_fresh || entry.checked.elapsed() < MAX_AGE)
        })
        .map(|entry| entry.binary.clone())
}

pub(super) fn remember(
    target: &RemoteTarget,
    config: &RemoteConfig,
    path: String,
    family: bootstrap::RemoteFamily,
) -> Result<RemoteBinary, String> {
    validate_remote_executable_token(&path)?;
    let binary = RemoteBinary { path, family };
    let remote = ResolvedRemote::resolve(target, config);
    if let Ok(mut cache) = CACHE.lock() {
        // Expiry only controls whether discovery may reuse a hint. Reconnect deliberately reads
        // stale hints, so refreshing one destination must not discard another destination's last
        // known executable. The roster remains bounded below.
        cache.retain(|entry| entry.remote != remote);
        if cache.len() >= 128 {
            cache.remove(0);
        }
        cache.push(CachedBinary {
            remote,
            binary: binary.clone(),
            checked: Instant::now(),
        });
    }
    Ok(binary)
}

pub(crate) fn invalidate(target: &RemoteTarget, config: &RemoteConfig) {
    let remote = ResolvedRemote::resolve(target, config);
    if let Ok(mut cache) = CACHE.lock() {
        cache.retain(|entry| entry.remote != remote);
    }
}

/// Find a compatible executable without ever installing or honoring an upload override.
pub(crate) fn resolve(
    target: &RemoteTarget,
    config: &RemoteConfig,
) -> Result<RemoteBinary, String> {
    super::validate_remote_target(target)?;
    if let Some(path) = cached(target, config) {
        return Ok(path);
    }
    let report = bootstrap::probe_remote_report(target, config)?;
    match bootstrap::select_compatible(&report) {
        bootstrap::ProbeResult::Found { path, .. } => {
            remember(target, config, path, report.remote_family())
        }
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
        remember(
            &target,
            &config,
            "/home/u/.local/bin/rozi".into(),
            bootstrap::RemoteFamily::Posix,
        )
        .unwrap();
        assert_eq!(
            cached(&target, &config).map(|binary| binary.path),
            Some("/home/u/.local/bin/rozi".to_string())
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
        assert!(
            remember(
                &target,
                &config,
                "rozi\nnext".into(),
                bootstrap::RemoteFamily::Posix
            )
            .is_err()
        );
        assert!(cached(&target, &config).is_none());
    }

    #[test]
    fn reconnect_keeps_a_stale_cached_binary() {
        let target = RemoteTarget::Alias("stale-binary-cache-fixture".into());
        let other_target = RemoteTarget::Alias("other-binary-cache-fixture".into());
        let config = RemoteConfig::default();
        remember(
            &target,
            &config,
            "/home/u/.local/bin/rozi".into(),
            bootstrap::RemoteFamily::Posix,
        )
        .unwrap();
        expire(&target, &config);
        assert!(
            cached(&target, &config).is_none(),
            "fresh lookups must still expire"
        );
        remember(
            &other_target,
            &config,
            "/opt/rozi/bin/rozi".into(),
            bootstrap::RemoteFamily::Posix,
        )
        .unwrap();
        assert_eq!(
            last_known(&target, &config).map(|binary| binary.path),
            Some("/home/u/.local/bin/rozi".to_string())
        );
        invalidate(&target, &config);
        invalidate(&other_target, &config);
        assert!(last_known(&target, &config).is_none());
    }

    fn expire(target: &RemoteTarget, config: &RemoteConfig) {
        let remote = ResolvedRemote::resolve(target, config);
        let Ok(mut cache) = CACHE.lock() else {
            return;
        };
        let stale = Instant::now()
            .checked_sub(MAX_AGE + Duration::from_secs(1))
            .expect("monotonic clock can represent a minute ago");
        for entry in cache.iter_mut() {
            if entry.remote == remote {
                entry.checked = stale;
            }
        }
    }
}
