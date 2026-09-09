//! Exercise the real POSIX probe/upload scripts through a local OpenSSH transport stand-in.
//! The stand-in joins argv exactly as ssh does; no network or developer directories are used.
#![cfg(unix)]

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use rozi::config::{RemoteConfig, RemoteInstallPolicy};
use rozi::session::discovery::{SessionSource, discover_sessions_from};
use rozi::session::remote::{RemoteTarget, ensure_remote_binary};

fn executable(path: &Path, bytes: &[u8]) {
    std::fs::write(path, bytes).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

fn run_case(case: &str) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("remote");
    let bin = root.path().join("transport");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    executable(
        &bin.join("ssh"),
        br##"#!/bin/sh
while [ "$#" -gt 0 ] && [ "$1" != -- ]; do shift; done
shift
shift
unset ROZI_ASKPASS_ENDPOINT ROZI_ASKPASS_TOKEN ROZI_ASKPASS_SESSION SSH_ASKPASS SSH_ASKPASS_REQUIRE
exec /bin/sh -c "$*"
"##,
    );
    if case == "discovery" {
        let cargo = home.join(".cargo/bin");
        std::fs::create_dir_all(&cargo).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_rozi"), cargo.join("rozi")).unwrap();
    }
    if case == "symlink" {
        std::fs::create_dir_all(home.join(".local/bin")).unwrap();
        std::os::unix::fs::symlink(home.join("untouched"), home.join(".local/bin/rozi")).unwrap();
    }
    let tui = case.starts_with("tui_");
    let mut child = if tui {
        tui_command(root.path(), case)
    } else {
        let mut command = Command::new(std::env::current_exe().unwrap());
        command.args(["--exact", "bootstrap_child", "--nocapture"]);
        command
    };
    child
        .env("ROZI_BOOTSTRAP_CASE", case)
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI_BIN")
        .env_remove("ROZI_REMOTE_BINARY");
    if matches!(case, "upload" | "symlink") {
        child.env("ROZI_REMOTE_BINARY", env!("CARGO_BIN_EXE_rozi"));
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if tui {
        let frame = std::fs::read_to_string(root.path().join("frame.md")).unwrap();
        let installed = home.join(".local/bin/rozi").is_file();
        assert_eq!(installed, case == "tui_accept", "{frame}");
        assert!(!frame.contains("Install Rozi on remote"), "{frame}");
        if installed {
            assert!(
                frame.contains("label: `open`"),
                "host discovery finished: {frame}"
            );
        }
    }
}

fn tui_command(root: &Path, case: &str) -> Command {
    let config = root.join("rozi.toml");
    std::fs::write(
        &config,
        "[session]\nstartup = \"picker\"\n[remote.hosts.fixture]\nhost = \"fixture\"\n",
    )
    .unwrap();
    let answer = if case == "tui_accept" {
        "type:yes; key:enter"
    } else {
        "key:esc"
    };
    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command
        .arg("--config")
        .arg(config)
        .env("TUI_LIPAN_SNAPSHOT", root.join("frame.md"))
        .env("TUI_LIPAN_SNAPSHOT_DIAGNOSTIC", "1")
        .env("TUI_LIPAN_SNAPSHOT_SETTLE_MS", "500")
        .env(
            "TUI_LIPAN_SNAPSHOT_SCRIPT",
            format!("key:ctrl+r; sleep:200; key:enter; sleep:1000; {answer}; sleep:2000"),
        );
    command
}

#[test]
fn tui_confirmation_installs_and_finishes_host_discovery() {
    run_case("tui_accept");
}

#[test]
fn tui_cancellation_leaves_the_host_untouched() {
    run_case("tui_decline");
}

#[test]
fn discovery_finds_a_cargo_install_without_path_configuration() {
    run_case("discovery");
}

#[test]
fn upload_is_shell_quoted_verified_and_discoverable() {
    run_case("upload");
}

#[test]
fn background_discovery_never_installs_even_with_always_policy() {
    run_case("readonly");
}

#[test]
fn upload_refuses_a_dangling_destination_symlink() {
    run_case("symlink");
}

#[test]
fn bootstrap_child() {
    let Ok(case) = std::env::var("ROZI_BOOTSTRAP_CASE") else {
        return;
    };
    let target = RemoteTarget::Alias("fixture".into());
    let config = RemoteConfig {
        install: RemoteInstallPolicy::Always,
        ..RemoteConfig::default()
    };
    let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap());
    match case.as_str() {
        "upload" => {
            let path = ensure_remote_binary(&target, &config, true).unwrap();
            assert_eq!(Path::new(&path), home.join(".local/bin/rozi"));
            assert_eq!(
                std::fs::read(path).unwrap(),
                std::fs::read(env!("CARGO_BIN_EXE_rozi")).unwrap()
            );
            assert!(
                discover_sessions_from(&SessionSource::Remote(target), &config)
                    .unwrap()
                    .is_empty()
            );
        }
        "discovery" => {
            assert!(
                discover_sessions_from(&SessionSource::Remote(target.clone()), &config)
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                ensure_remote_binary(&target, &config, false).unwrap(),
                home.join(".cargo/bin/rozi").to_str().unwrap()
            );
            assert!(!home.join(".local/bin/rozi").exists());
        }
        "readonly" => {
            assert!(
                discover_sessions_from(&SessionSource::Remote(target.clone()), &config).is_err()
            );
            assert!(ensure_remote_binary(&target, &config, false).is_err());
            assert!(!home.join(".local").exists());
        }
        "symlink" => {
            assert!(
                ensure_remote_binary(&target, &config, true)
                    .unwrap_err()
                    .contains("refuse_non_regular")
            );
            assert!(!home.join("untouched").exists());
            assert!(
                home.join(".local/bin/rozi")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        _ => panic!("unknown fixture"),
    }
}
