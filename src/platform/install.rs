//! Thin hyprmux adapters over `relswap::install`.
//!
//! Path policy (`PlatformEnv` / XDG) stays here; the durable activation engine lives in `relswap`.

use crate::platform::paths::{self, PlatformEnv};
use crate::release_app::HYPRMUX;
use relswap::{Installation, NoFaultInjector, UreqDownloader};

/// Production constructor using the platform's managed data and command paths.
pub fn from_platform_env(env: &PlatformEnv) -> Installation<UreqDownloader> {
    Installation::new(
        &HYPRMUX,
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

pub use relswap::{
    ActivationBoundary, ActivationResult, CheckResult, FaultInjector, FaultPoint, InstallError,
    InstallState, LauncherMetadata, LauncherOwnership, Manager, PendingActivation, VersionState,
};
