//! Thin rozi adapters over `relswap::install`.
//!
//! Path policy (`PlatformEnv` / XDG) stays here; the durable activation engine lives in `relswap`.

use std::sync::Arc;

use crate::platform::paths::{self, PlatformEnv};
use crate::release_app::ROZI;
use relswap::{Installation, NoFaultInjector, ProgressObserver, UreqDownloader};

/// Production constructor using the platform's managed data and command paths.
pub fn from_platform_env(env: &PlatformEnv) -> Installation<UreqDownloader> {
    Installation::new(
        &ROZI,
        paths::data_dir(env),
        paths::managed_command_path(env),
        UreqDownloader::new(),
        NoFaultInjector,
    )
}

/// Production constructor using the process environment snapshot.
pub fn from_process() -> Installation<UreqDownloader> {
    from_platform_env(&PlatformEnv::from_process())
}

/// A managed installation whose downloads report progress to `observer`.
///
/// Same engine and same transport as [`from_platform_env`] - only the reporting differs. That is
/// the point of taking this from `relswap` rather than implementing `Downloader` here: a local
/// implementation would also own a copy of relswap's TLS, redirect, and timeout policy, and
/// getting `RootCerts::PlatformVerifier` wrong in such a copy is what broke every managed install
/// in 0.0.3.
pub fn from_platform_env_with_progress(
    env: &PlatformEnv,
    observer: Arc<dyn ProgressObserver>,
) -> Installation<UreqDownloader> {
    Installation::new(
        &ROZI,
        paths::data_dir(env),
        paths::managed_command_path(env),
        UreqDownloader::with_progress(observer),
        NoFaultInjector,
    )
}

/// Progress-reporting constructor using the process environment snapshot.
pub fn from_process_with_progress(
    observer: Arc<dyn ProgressObserver>,
) -> Installation<UreqDownloader> {
    from_platform_env_with_progress(&PlatformEnv::from_process(), observer)
}

/// Phrases a failed download carries when the machine simply is not online. `relswap` reports
/// every download failure as one string, so this is the only way to tell "offline" from a bad
/// response.
const OFFLINE_MARKERS: &[&str] = &[
    "name resolution",
    "lookup address",
    "failed to lookup",
    "network is unreachable",
    "connection refused",
    "connection reset",
    "timed out",
    "no route to host",
];

/// What went wrong, for a person: being offline reads as an instruction instead of a stack of
/// nested library errors; anything else keeps its detail.
pub fn describe_error(error: &InstallError) -> String {
    if let InstallError::Release(relswap::ReleaseError::Download(detail)) = error {
        let detail = detail.to_ascii_lowercase();
        if OFFLINE_MARKERS.iter().any(|marker| detail.contains(marker)) {
            return "could not reach the release server - check your internet connection and try again"
                .to_string();
        }
    }
    error.to_string()
}

pub use relswap::{
    ActivationBoundary, ActivationResult, CheckResult, FaultInjector, FaultPoint, InstallError,
    InstallState, LauncherMetadata, LauncherOwnership, Manager, PendingActivation, VersionState,
};
