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

#[cfg(test)]
mod tests {
    use super::ROZI;
    use relswap::TrustedKeySet;

    /// The trust anchor is a JSON file in this repository that is compiled into every binary, and
    /// `relswap` fails closed on one it cannot parse or one that carries no usable key. Both of
    /// those are silent at build time: the binary still links, still runs, and only reveals the
    /// mistake when a user's update refuses a perfectly good release. A hand-edit during a key
    /// rotation is exactly when that is most likely, and exactly when nobody is looking here.
    #[test]
    fn the_compiled_trust_anchor_can_verify_a_release() {
        let keys = TrustedKeySet::from_bytes(ROZI.trust_anchor)
            .expect("release-keys.json parses as a trust anchor");
        assert!(
            keys.has_ed25519_key(),
            "no Ed25519 key remains in release-keys.json; every update would fail closed"
        );
    }

    /// Rotation replaces the key a release is *signed* with, but a binary only trusts what was
    /// compiled into it. A key added today therefore reaches an installed rozi only through an
    /// update signed by a key it already has - so the trust store must be able to hold more than
    /// one, and the retiring key must outlive the release that introduces its replacement.
    /// Retiring the old key in the same release that adds the new one strands every install.
    #[test]
    fn every_trusted_key_is_distinct_and_usable() {
        let keys = TrustedKeySet::from_bytes(ROZI.trust_anchor).expect("trust anchor parses");
        let mut ids: Vec<&str> = keys.keys.iter().map(|key| key.id.as_str()).collect();
        ids.sort_unstable();
        let total = ids.len();
        ids.dedup();
        assert_eq!(total, ids.len(), "release-keys.json repeats a key id");
        assert!(
            keys.keys.iter().all(|key| !key.id.is_empty()),
            "a trusted key has an empty id"
        );
    }

    /// `relswap` reads this to decide which release it is running as, and the release workflow
    /// refuses to publish a tag that disagrees with `Cargo.toml`. Keeping the two spellings in one
    /// place means `rozi install` can never ask for a version this binary is not.
    #[test]
    fn the_app_version_is_the_package_version() {
        assert_eq!(ROZI.version, env!("CARGO_PKG_VERSION"));
    }
}
