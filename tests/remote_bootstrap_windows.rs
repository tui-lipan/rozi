//! Exercise the Windows SCP/PowerShell bootstrap transaction with native helper transports.
#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Command;

use rozi::config::{RemoteConfig, RemoteInstallPolicy};
use rozi::session::remote::{RemoteTarget, ensure_remote_binary};

fn write_transport_scripts(bin: &Path) {
    let source = bin.join("fake_transport.rs");
    std::fs::write(
        &source,
        r#"use std::path::PathBuf;
use std::process::Command;

fn main() {
    let program = std::env::current_exe().unwrap();
    let name = program.file_stem().unwrap().to_string_lossy().to_ascii_lowercase();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    if name == "scp" {
        let source = &args[args.len() - 2];
        let destination = &args[args.len() - 1];
        let remote_name = destination.split_once(':').unwrap().1;
        std::fs::copy(source, home.join(remote_name)).unwrap();
        return;
    }
    let marker = args.iter().position(|arg| arg == "--").unwrap();
    let remote = &args[marker + 2..];
    let requested = PathBuf::from(&remote[0]);
    let executable = if requested.is_relative() && (remote[0].contains('\\') || remote[0].contains('/')) {
        home.join(requested)
    } else {
        requested
    };
    let status = Command::new(executable)
        .args(&remote[1..])
        .current_dir(home)
        .status()
        .unwrap();
    std::process::exit(status.code().unwrap_or(1));
}
"#,
    )
    .unwrap();
    let output = Command::new("rustc")
        .arg(&source)
        .args(["-o"])
        .arg(bin.join("ssh.exe"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::copy(bin.join("ssh.exe"), bin.join("scp.exe")).unwrap();
}

fn managed(home: &Path) -> PathBuf {
    home.join(".local/share/rozi/remote")
        .join(env!("CARGO_PKG_VERSION"))
        .join("rozi.exe")
}

fn run_case(case: &str) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("remote home with spaces");
    let bin = root.path().join("transport");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    write_transport_scripts(&bin);

    let final_path = managed(&home);
    if case != "upload" {
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    }
    match case {
        "existing" => {
            std::fs::copy(env!("CARGO_BIN_EXE_rozi"), &final_path).unwrap();
            std::fs::remove_file(bin.join("scp.exe")).unwrap();
        }
        "directory" => std::fs::create_dir(&final_path).unwrap(),
        "reparse" => {
            let untouched = home.join("untouched.exe");
            std::fs::copy(env!("CARGO_BIN_EXE_rozi"), &untouched).unwrap();
            std::os::windows::fs::symlink_file(&untouched, &final_path).unwrap();
        }
        "failed_stage" => {
            std::fs::write(root.path().join("invalid-rozi.exe"), b"not an exe").unwrap()
        }
        "upload" => {}
        _ => panic!("unknown case"),
    }

    let mut paths = vec![bin];
    paths.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "bootstrap_windows_child", "--nocapture"])
        .env("ROZI_BOOTSTRAP_WINDOWS_CASE", case)
        .env("USERPROFILE", &home)
        .env("HOME", &home)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("XDG_CONFIG_HOME", root.path().join("config"))
        .env("XDG_STATE_HOME", root.path().join("state"))
        .env("XDG_DATA_HOME", root.path().join("data"))
        .env("XDG_CACHE_HOME", root.path().join("cache"))
        .env_remove("ROZI_CONFIG")
        .env_remove("ROZI_SOCKET")
        .env_remove("ROZI_BIN")
        .env_remove("ROZI_REMOTE_BINARY");
    if case == "failed_stage" {
        child.env("ROZI_REMOTE_BINARY", root.path().join("invalid-rozi.exe"));
    } else if case != "existing" {
        child.env("ROZI_REMOTE_BINARY", env!("CARGO_BIN_EXE_rozi"));
    }
    let output = child.output().unwrap();
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn windows_upload_finalizes_in_a_home_path_with_spaces() {
    run_case("upload");
}

#[test]
fn windows_probe_reuses_an_existing_managed_runtime() {
    run_case("existing");
}

#[test]
fn windows_refuses_a_directory_target() {
    run_case("directory");
}

#[test]
fn windows_refuses_a_reparse_target() {
    run_case("reparse");
}

#[test]
fn windows_failed_verification_cleans_the_uploaded_temporary() {
    run_case("failed_stage");
}

#[test]
fn bootstrap_windows_child() {
    let Ok(case) = std::env::var("ROZI_BOOTSTRAP_WINDOWS_CASE") else {
        return;
    };
    let target = RemoteTarget::Alias("fixture".into());
    let config = RemoteConfig {
        install: RemoteInstallPolicy::Always,
        ..RemoteConfig::default()
    };
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    let final_path = managed(&home);

    match case.as_str() {
        "upload" => {
            let path = ensure_remote_binary(&target, &config, true).unwrap();
            assert_eq!(
                path,
                format!(
                    ".local\\share\\rozi\\remote\\{}\\rozi.exe",
                    env!("CARGO_PKG_VERSION")
                )
            );
            assert_eq!(
                std::fs::read(final_path).unwrap(),
                std::fs::read(env!("CARGO_BIN_EXE_rozi")).unwrap()
            );
        }
        "existing" => {
            let path = ensure_remote_binary(&target, &config, true).unwrap();
            assert!(path.starts_with(".local\\share\\rozi\\remote\\"));
        }
        "directory" | "reparse" => {
            let error = ensure_remote_binary(&target, &config, true).unwrap_err();
            assert!(error.contains("refuse_non_regular"), "{error}");
        }
        "failed_stage" => {
            assert!(ensure_remote_binary(&target, &config, true).is_err());
            let leftovers = std::fs::read_dir(&home)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_string_lossy()
                        .starts_with("rozi.install.")
                })
                .count();
            assert_eq!(
                leftovers, 0,
                "failed finalization left an uploaded temporary"
            );
            assert!(!final_path.exists());
        }
        _ => panic!("unknown case"),
    }
}
