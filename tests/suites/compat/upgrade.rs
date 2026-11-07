//! `rozi sessions upgrade <NAME>`, which WS4 adds.
//!
//! Until that command parses, this test skips with that reason and does not download a release.
//! Once it exists, the flow runs only when both binaries are generation 52 or newer: start on the
//! released binary, upgrade onto the built one, and read the restored panes back.

use super::harness::{self, Mode, pair};

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release once sessions upgrade exists"]
fn sessions_upgrade_restarts_onto_the_new_binary() {
    if !harness::skip_unless_enabled() {
        return;
    }
    let new_bin = std::path::PathBuf::from(env!("CARGO_BIN_EXE_rozi"));
    if !upgrade_command_exists(&new_bin) {
        eprintln!("skipping: WS4 has not added `rozi sessions upgrade` yet");
        return;
    }
    let pair = pair();
    if pair.mode() != Mode::Window {
        eprintln!(
            "skipping: upgrade flow waits until both binaries are generation >= {} (released {}, built {})",
            harness::SUPPORT_WINDOW_GENERATION,
            pair.old_protocol,
            pair.new_protocol
        );
        return;
    }

    let mut session = harness::Session::start(&pair.old_bin);
    let sleeper = harness::sleeper_argv();
    let mut args = vec!["split", "--argv"];
    args.extend(sleeper);
    let split = session.client(&pair.old_bin, &args);
    assert!(
        split.success(),
        "old server did not open a pane to upgrade:\n{}",
        split.combined()
    );

    let upgraded = harness::run(
        &pair.new_bin,
        &session.env,
        &["sessions", "upgrade", &session.name],
    );
    assert!(
        upgraded.success(),
        "sessions upgrade failed:\n{}",
        upgraded.combined()
    );
    session.kill_with = pair.new_bin.clone();

    let restored = session.list_panes(&pair.new_bin);
    assert!(
        !harness::pane_ids(&restored).is_empty(),
        "upgrade came back with no panes:\n{}",
        restored.combined()
    );
}

fn upgrade_command_exists(bin: &std::path::Path) -> bool {
    let env = harness::Isolated::new();
    let probe = harness::run(bin, &env, &["sessions", "upgrade"]);
    !probe.combined().contains("unknown sessions command")
}
