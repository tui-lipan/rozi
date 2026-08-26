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
    // The budget covers a *cold* first execution, not a warm one.  The probe runs the payload
    // immediately after writing it, so on Windows an unsigned ~18 MB binary meets real-time
    // protection at its least informed: a full scan of a file nothing has seen before, plus a
    // cloud-delivered protection lookup that blocks for up to 10 seconds on its own.  A budget of
    // 10 seconds therefore sat exactly on Defender's own ceiling and failed the install on a
    // machine that was working correctly - the retry then passed, because by then the file was
    // cached.  A generous ceiling costs a healthy payload nothing: `--version` exits as soon as it
    // has printed, and the probe waits on the process, not on the clock.
    self_test: Some(relswap::SelfTest {
        args: &["--version"],
        timeout: std::time::Duration::from_secs(90),
    }),
};
