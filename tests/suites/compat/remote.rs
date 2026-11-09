//! The same cross-version checks over `ssh localhost`.
//!
//! The server is started locally in the lab. The client reaches it with `--remote localhost`, and
//! `[remote.hosts.localhost] binary_path` is a wrapper that execs the server binary with the lab's
//! directories. That keeps the remote process out of the developer's runtime, and it skips the
//! install probe. Skipped unless key-based ssh to localhost already works. `ROZI_REQUIRE_SSH_TEST=1`
//! makes a missing ssh setup a failure, which is what the nightly and release-health jobs set.

#![cfg(unix)]

use std::path::Path;

use super::flows;
use super::harness::{self, Mode, Session, Transport, pair};

#[test]
#[ignore = "set ROZI_COMPAT=1; downloads a signed release; needs ssh to localhost"]
fn localhost_ssh_runs_the_cross_version_flows() {
    if !harness::skip_unless_enabled() || !localhost_ssh_available() {
        return;
    }
    let pair = pair();
    match pair.mode() {
        Mode::Window => {
            flows::exercise(&pair.old_bin, &pair.new_bin, Transport::Ssh);
            flows::exercise(&pair.new_bin, &pair.old_bin, Transport::Ssh);
        }
        // A control command over `--remote` is answered by the host's own rozi, which speaks the
        // session wire with its local server and only the control API with this client. Neither
        // side ever sees the other's generation, so a forwarded command works across it.
        Mode::Mismatch | Mode::Same => {
            remote_lists(&pair.old_bin, &pair.new_bin);
            remote_lists(&pair.new_bin, &pair.old_bin);
        }
    }
}

fn remote_lists(server: &Path, client: &Path) {
    let session = prepared(server);
    let listed = session.call(client, Transport::Ssh, &["list-panes", "--format", "json"]);
    assert!(
        harness::pane_ids(&listed).is_empty(),
        "ssh list-panes failed:\n{}",
        listed.combined()
    );
}

fn prepared(server: &Path) -> Session {
    let session = Session::start(server);
    session.env.pin_localhost_binary(server);
    session
}

/// Whether `ssh localhost` completes without a prompt. Anything else means "not set up here".
fn localhost_ssh_available() -> bool {
    let available = rozi::platform::command::program_exists("ssh")
        && std::process::Command::new("ssh")
            .args([
                "-o",
                "BatchMode=yes",
                "-o",
                "ConnectTimeout=5",
                "-o",
                "StrictHostKeyChecking=yes",
                "localhost",
                "--",
                "true",
            ])
            .output()
            .is_ok_and(|out| out.status.success());
    assert!(
        available || std::env::var("ROZI_REQUIRE_SSH_TEST").as_deref() != Ok("1"),
        "ROZI_REQUIRE_SSH_TEST=1 requires key-based ssh to localhost and a trusted host key"
    );
    if !available {
        eprintln!("skipping: key-based ssh to localhost is not available");
    }
    available
}
