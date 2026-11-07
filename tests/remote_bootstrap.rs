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

/// A remote rozi of another release that speaks this build's protocol range, answering the probe
/// the way a real binary does: `--version` lines, and `--remote` in `--help`.
fn fake_release(path: &Path, version: &str) {
    let real = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .arg("--version")
        .output()
        .unwrap();
    let range: String = String::from_utf8(real.stdout)
        .unwrap()
        .lines()
        .filter(|line| line.starts_with("protocol_"))
        .map(|line| format!("{line}\\n"))
        .collect();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    executable(
        path,
        format!(
            "#!/bin/sh\ncase \"$1\" in\n--version) printf 'rozi {version}\\nextension_api=1\\n{range}' ;;\n--help) echo '  --remote <HOST>' ;;\nesac\n"
        )
        .as_bytes(),
    );
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
cd "$HOME"
printf '%s\n' "$*" >> "$ROZI_TEST_SSH_LOG"
exec /bin/sh -c "$*"
"##,
    );
    if matches!(case, "discovery" | "tui_attach_missing") {
        let cargo = home.join(".cargo/bin");
        std::fs::create_dir_all(&cargo).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_rozi"), cargo.join("rozi")).unwrap();
    }
    let data_home = if case == "upload" {
        root.path().join("data home with spaces")
    } else {
        root.path().join("data")
    };
    let managed = data_home
        .join("rozi/remote")
        .join(env!("CARGO_PKG_VERSION"))
        .join("rozi");
    if matches!(case, "symlink" | "staged_failure") {
        std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
    }
    if case == "symlink" {
        std::os::unix::fs::symlink(home.join("untouched"), &managed).unwrap();
    }
    if case == "staged_failure" {
        std::fs::write(&managed, b"previous managed runtime").unwrap();
        executable(&root.path().join("invalid-rozi"), b"#!/bin/sh\nexit 1\n");
    }
    if case == "prefer_exact" {
        // A newer and an older release sit earlier in probe order than the exact copy.
        fake_release(&home.join(".local/bin/rozi"), "999.0.0");
        fake_release(&home.join(".cargo/bin/rozi"), "0.0.1");
        std::fs::create_dir_all(managed.parent().unwrap()).unwrap();
        std::fs::copy(env!("CARGO_BIN_EXE_rozi"), &managed).unwrap();
    }
    if case == "prefer_newest" {
        fake_release(&home.join(".local/bin/rozi"), "900.0.0");
        fake_release(&home.join(".cargo/bin/rozi"), "900.1.0");
        fake_release(&home.join("bin/rozi"), "dev-build");
    }
    if case == "tui_always" {
        let global = home.join(".local/bin/rozi");
        std::fs::create_dir_all(global.parent().unwrap()).unwrap();
        executable(&global, b"#!/bin/sh\necho incompatible\n");
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
        .env("ROZI_TEST_SSH_LOG", root.path().join("ssh.log"))
        .env("ROZI_BOOTSTRAP_CASE", case)
        .env("HOME", &home)
        .env("PATH", format!("{}:/usr/bin:/bin", bin.display()))
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_DATA_HOME", &data_home)
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env("XDG_RUNTIME_DIR", root.path().join("run"))
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI_BIN")
        .env_remove("ROZI_REMOTE_BINARY");
    if matches!(case, "upload" | "symlink") {
        child.env("ROZI_REMOTE_BINARY", env!("CARGO_BIN_EXE_rozi"));
    } else if case == "staged_failure" {
        child.env("ROZI_REMOTE_BINARY", root.path().join("invalid-rozi"));
    }
    let output = child.output().unwrap();
    if case == "tui_attach_missing" {
        // Clean up even when testing a broken dispatcher that did start the missing server.
        let mut cleanup = Command::new(env!("CARGO_BIN_EXE_rozi"));
        for (key, value) in child.get_envs() {
            if let Some(value) = value {
                cleanup.env(key, value);
            } else {
                cleanup.env_remove(key);
            }
        }
        let spawned = root.path().join("run/rozi/session-missing.sock").exists();
        cleanup
            .args(["sessions", "kill", "missing"])
            .output()
            .unwrap();
        let commands = std::fs::read_to_string(root.path().join("ssh.log")).unwrap();
        assert!(
            !spawned,
            "existing-only attach started a server: {commands}"
        );
        assert!(commands.contains("--remote-serve-existing"), "{commands}");
        let frame = std::fs::read_to_string(root.path().join("frame.md")).unwrap();
        assert!(
            frame.contains("Session missing is not running on the remote host"),
            "{frame}"
        );
        assert!(
            !frame.contains("original remote session"),
            "initial attach must not report session loss: {frame}"
        );
    }
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if tui && case != "tui_attach_missing" {
        let frame = std::fs::read_to_string(root.path().join("frame.md")).unwrap();
        let installed = managed.is_file();
        assert_eq!(
            installed,
            matches!(case, "tui_accept" | "tui_always"),
            "{frame}"
        );
        assert!(!frame.contains("Install Rozi on remote"), "{frame}");
        if installed {
            assert!(
                frame.contains("title: `Sessions`") && !frame.contains("title: `Remote hosts`"),
                "host discovery opens the shared Sessions picker: {frame}"
            );
        }
        if case == "tui_always" {
            assert_eq!(
                std::fs::read(home.join(".local/bin/rozi")).unwrap(),
                b"#!/bin/sh\necho incompatible\n"
            );
        }
    }
}

fn tui_command(root: &Path, case: &str) -> Command {
    let config = root.join("rozi.toml");
    if case == "tui_attach_missing" {
        std::fs::write(
            &config,
            format!(
                "[remote]\ninstall = \"never\"\n[remote.hosts.fixture]\nhost = \"fixture\"\nbinary_path = {:?}\n",
                root.join("remote/.cargo/bin/rozi").to_str().unwrap()
            ),
        ).unwrap();
        let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
        command
            .arg("--config")
            .arg(config)
            .args(["--remote", "fixture", "sessions", "attach", "missing"])
            .env("TUI_LIPAN_SNAPSHOT", root.join("frame.md"))
            .env("TUI_LIPAN_SNAPSHOT_DIAGNOSTIC", "1")
            .env("TUI_LIPAN_SNAPSHOT_SETTLE_MS", "2000");
        return command;
    }
    std::fs::write(
        &config,
        format!(
            "[session]\nstartup = \"picker\"\n[remote]\ninstall = \"{}\"\n[remote.hosts.fixture]\nhost = \"fixture\"\n",
            if case == "tui_always" { "always" } else { "prompt" }
        ),
    )
    .unwrap();
    // A slow runner can take seconds to connect and install, so each step waits for the screen it
    // needs rather than for a fixed time. The Sessions picker is already open under the host list
    // at startup, so only its footer for a connected host says discovery has finished.
    const PROMPT: &str = "wait-for:exists,text~Install Rozi on remote,30000";
    const PICKER: &str = "wait-for:exists,text~disconnect host,30000";
    let answer = if case == "tui_accept" {
        format!("{PROMPT}; type:yes; key:enter; {PICKER}")
    } else if case == "tui_always" {
        PICKER.to_string()
    } else {
        format!("{PROMPT}; key:esc; sleep:2000")
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
            format!(
                "key:ctrl+r; wait-for:exists,text~Search hosts,30000; key:enter; {answer}; wait:500"
            ),
        );
    command
}

#[test]
fn remote_sessions_attach_missing_never_starts_a_server() {
    run_case("tui_attach_missing");
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
fn tui_always_policy_installs_without_opening_confirmation() {
    run_case("tui_always");
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
fn failed_staged_verification_preserves_the_previous_runtime() {
    run_case("staged_failure");
}

#[test]
fn the_host_copy_of_this_exact_version_wins_over_newer_and_older_releases() {
    run_case("prefer_exact");
}

#[test]
fn without_an_exact_copy_the_newest_compatible_release_wins() {
    run_case("prefer_newest");
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
            assert_eq!(
                Path::new(&path),
                std::path::Path::new(&std::env::var_os("XDG_DATA_HOME").unwrap())
                    .join("rozi/remote")
                    .join(env!("CARGO_PKG_VERSION"))
                    .join("rozi")
            );
            assert_eq!(
                std::fs::read(home.join(path)).unwrap(),
                std::fs::read(env!("CARGO_BIN_EXE_rozi")).unwrap()
            );
            assert!(
                discover_sessions_from(&SessionSource::Remote(target), &config)
                    .unwrap()
                    .is_empty()
            );
        }
        "prefer_exact" => {
            let path = ensure_remote_binary(&target, &config, false).unwrap();
            assert_eq!(
                Path::new(&path),
                std::path::Path::new(&std::env::var_os("XDG_DATA_HOME").unwrap())
                    .join("rozi/remote")
                    .join(env!("CARGO_PKG_VERSION"))
                    .join("rozi")
            );
        }
        "prefer_newest" => {
            assert_eq!(
                ensure_remote_binary(&target, &config, false).unwrap(),
                home.join(".cargo/bin/rozi").to_str().unwrap()
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
                std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap())
                    .join("rozi/remote")
                    .join(env!("CARGO_PKG_VERSION"))
                    .join("rozi")
                    .symlink_metadata()
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
        "staged_failure" => {
            assert!(ensure_remote_binary(&target, &config, true).is_err());
            assert_eq!(
                std::fs::read(
                    std::path::PathBuf::from(std::env::var_os("XDG_DATA_HOME").unwrap())
                        .join("rozi/remote")
                        .join(env!("CARGO_PKG_VERSION"))
                        .join("rozi")
                )
                .unwrap(),
                b"previous managed runtime"
            );
        }
        _ => panic!("unknown fixture"),
    }
}
