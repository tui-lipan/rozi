//! Rozi product identity for the `relswap` managed-install engine.

use relswap::{ActivationStrategy, App};

/// Compiled trust anchor and activation policy for rozi releases.
///
/// The trust anchor bytes live in this repo (`release-keys.json`); `relswap` never embeds a key.
pub const ROZI: App = App {
    name: "rozi",
    version: env!("CARGO_PKG_VERSION"),
    repository_url: "https://github.com/tui-lipan/rozi/",
    trust_anchor: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/release-keys.json")),
    activation: ActivationStrategy::WindowsLauncher {
        launcher_name: "rozi-launcher.exe",
        protocol: 1,
    },
    // `relswap` requires the probe output to contain the version being activated, which during an
    // update is the staged release rather than this binary's own version.  `rozi --version`
    // prints `rozi <version>` as its first line.
    self_test: Some(relswap::SelfTest {
        args: &["--version"],
        timeout: std::time::Duration::from_secs(10),
    }),
};
