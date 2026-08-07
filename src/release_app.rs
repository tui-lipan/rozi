//! Hyprmux product identity for the `relswap` managed-install engine.

use relswap::{ActivationStrategy, App};

/// Compiled trust anchor and activation policy for hyprmux releases.
///
/// The trust anchor bytes live in this repo (`release-keys.json`); `relswap` never embeds a key.
pub const HYPRMUX: App = App {
    name: "hyprmux",
    version: env!("CARGO_PKG_VERSION"),
    repository_url: "https://github.com/Razuer/hyprmux/",
    trust_anchor: include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/release-keys.json")),
    activation: ActivationStrategy::WindowsLauncher {
        launcher_name: "hyprmux-launcher.exe",
        protocol: 1,
    },
    self_test: Some(relswap::SelfTest {
        args: &["--version"],
        expect_contains: env!("CARGO_PKG_VERSION"),
        timeout: std::time::Duration::from_secs(10),
    }),
};
