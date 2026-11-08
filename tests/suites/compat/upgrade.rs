//! `rozi sessions upgrade <NAME>`: start a session on the released binary, upgrade it onto the
//! built one, and read the restored panes back.
//!
//! It runs whether or not the two binaries share a generation. Inside the window the upgrade moves
//! a session the new client could already attach to; across a generation bump it is the only way
//! back into a session the new client is refused by, so both have to work.

use super::harness::{self, Mode, pair};

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release"]
fn sessions_upgrade_restarts_onto_the_new_binary() {
    if !harness::skip_unless_enabled() {
        return;
    }
    let pair = pair();
    if pair.mode() == Mode::Same {
        eprintln!(
            "skipping: released and built binaries are the same pre-window generation {}",
            pair.old_protocol
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
