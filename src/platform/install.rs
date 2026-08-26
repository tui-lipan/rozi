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

pub use relswap::{
    ActivationBoundary, ActivationResult, CheckResult, FaultInjector, FaultPoint, InstallError,
    InstallState, LauncherMetadata, LauncherOwnership, Manager, PendingActivation, VersionState,
};
