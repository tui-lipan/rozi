//! What a client and a server on different generations do today.
//!
//! Generation 52 is the first build that keeps a session up across a release. Until both binaries
//! advertise that generation, a newer client against the released server (and the reverse) must
//! fail the handshake with `protocol-mismatch`, leave the server running, and — for a new client
//! against the old server — come back through `sessions restart`, which restores the snapshot.

use std::path::Path;

use super::harness::{self, Mode, Pair, Session, assert_clean_protocol_mismatch, pair};

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release"]
fn same_generation_binaries_list_each_others_session() {
    if !harness::skip_unless_enabled() {
        return;
    }
    let pair = pair();
    if pair.mode() != Mode::Same {
        eprintln!(
            "skipping: released session_protocol {} and built {} are not the same pre-window generation",
            pair.old_protocol, pair.new_protocol
        );
        return;
    }
    assert_lists_empty(&pair.old_bin, &pair.new_bin);
    assert_lists_empty(&pair.new_bin, &pair.old_bin);
}

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release"]
fn mismatched_generations_refuse_attach_and_restart_from_the_snapshot() {
    if !harness::skip_unless_enabled() {
        return;
    }
    let pair = pair();
    if pair.mode() != Mode::Mismatch {
        eprintln!(
            "skipping: released session_protocol {} and built {} are not a pre-window mismatch",
            pair.old_protocol, pair.new_protocol
        );
        return;
    }
    refuse_and_leave_the_server_up(&pair.new_bin, &pair.old_bin, "old client, new server");
    restart_old_server_onto_the_new_binary(pair);
}

fn assert_lists_empty(server: &Path, client: &Path) {
    let session = Session::start(server);
    let listed = session.list_panes(client);
    assert!(
        harness::pane_ids(&listed).is_empty(),
        "a fresh session listed panes:\n{}",
        listed.combined()
    );
}

fn refuse_and_leave_the_server_up(server: &Path, client: &Path, label: &str) {
    let session = Session::start(server);
    let refused = session.list_panes(client);
    assert_clean_protocol_mismatch(&refused);
    let still = session.list_panes(server);
    assert!(
        still.success(),
        "{label}: the server stopped answering after the mismatched client:\n{}",
        still.combined()
    );
}

fn restart_old_server_onto_the_new_binary(pair: &Pair) {
    let mut session = Session::start(&pair.old_bin);
    let sleeper = harness::sleeper_argv();
    let mut args = vec!["split", "--argv"];
    args.extend(sleeper);
    let split = session.client(&pair.old_bin, &args);
    assert!(
        split.success(),
        "old server did not open a pane to snapshot:\n{}",
        split.combined()
    );
    let before = harness::pane_ids(&session.list_panes(&pair.old_bin));
    assert_eq!(
        before.len(),
        1,
        "snapshot needs the pane that was just opened"
    );

    let refused = session.list_panes(&pair.new_bin);
    assert_clean_protocol_mismatch(&refused);
    let alive = session.list_panes(&pair.old_bin);
    assert!(
        alive.success(),
        "old server exited when the new client was refused:\n{}",
        alive.combined()
    );
    assert_eq!(
        harness::pane_ids(&alive),
        before,
        "the mismatched client changed the server's panes"
    );

    let restarted = harness::run(
        &pair.new_bin,
        &session.env,
        &["sessions", "restart", &session.name],
    );
    assert!(
        restarted.success(),
        "sessions restart did not restore the old server's snapshot:\n{}",
        restarted.combined()
    );
    session.kill_with = pair.new_bin.clone();

    let restored = session.list_panes(&pair.new_bin);
    let panes = harness::pane_ids(&restored);
    assert!(
        !panes.is_empty(),
        "restart-from-snapshot came back with no panes:\n{}",
        restored.combined()
    );
}
