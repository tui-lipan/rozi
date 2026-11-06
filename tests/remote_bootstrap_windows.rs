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
use std::io::Write;

fn trace(event: &str) {
    let path = std::env::var_os("ROZI_TEST_TRANSPORT_TRACE").unwrap();
    let mut log = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
    writeln!(log, "{:?} pid={} {event}", std::time::SystemTime::now(), std::process::id()).unwrap();
}

fn main() {
    trace("transport entered");
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
    let requested = PathBuf::from(remote[0].trim_matches('"'));
    let executable = if requested.is_relative() && (remote[0].contains('\\') || remote[0].contains('/')) {
        home.join(requested)
    } else {
        requested
    };
    let started = std::time::Instant::now();
    // Delay only a full PowerShell script, after family detection has already succeeded: the
    // probe before anything is uploaded, or the finalization once the upload is staged.
    let script = remote.last().is_some_and(|arg| arg.len() > 1000);
    let staged = std::fs::read_dir(&home).unwrap().filter_map(Result::ok).any(|entry| {
        entry.file_name().to_string_lossy().starts_with("rozi.install.")
    });
    let delayed = match std::env::var("ROZI_BOOTSTRAP_WINDOWS_CASE").as_deref() {
        Ok("slow_probe") => script && !staged,
        Ok("slow_finalize") => script && staged,
        _ => false,
    };
    if delayed {
        trace("simulating 50-second remote startup");
        std::thread::sleep(std::time::Duration::from_secs(50));
    }
    let mut child = Command::new(executable)
        .args(&remote[1..])
        .current_dir(home)
        .spawn()
        .unwrap();
    trace(&format!("remote child {} spawned; waiting", child.id()));
    let status = child.wait().unwrap();
    trace(&format!("remote child exited {status} after {:?}", started.elapsed()));
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

fn managed(data_home: &Path) -> PathBuf {
    data_home
        .join("rozi")
        .join("remote")
        .join(env!("CARGO_PKG_VERSION"))
        .join("rozi.exe")
}

fn assert_same_path(reported: &str, expected: &Path) {
    assert_eq!(
        std::fs::canonicalize(reported).unwrap(),
        std::fs::canonicalize(expected).unwrap()
    );
}

fn run_case(case: &str) {
    let root = tempfile::tempdir().unwrap();
    let home = root.path().join("remote home with spaces");
    let data_home = root.path().join("local app data with spaces");
    let bin = root.path().join("transport");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&bin).unwrap();
    write_transport_scripts(&bin);

    let final_path = managed(&data_home);
    if !matches!(case, "upload" | "slow_finalize") {
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
    }
    match case {
        "existing" | "slow_probe" => {
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
        "upload" | "slow_finalize" => {}
        _ => panic!("unknown case"),
    }

    // Do not inherit Cargo's target directories: they contain another `rozi.exe`, which discovery
    // must correctly prefer over the managed fixture. The child only needs our transports and
    // Windows' built-in PowerShell/system commands.
    let windows = PathBuf::from(std::env::var_os("SystemRoot").unwrap());
    let paths = [
        bin,
        windows.join("System32/WindowsPowerShell/v1.0"),
        windows.join("System32"),
        windows,
    ];
    let mut child = Command::new(std::env::current_exe().unwrap());
    child
        .args(["--exact", "bootstrap_windows_child", "--nocapture"])
        .env("ROZI_BOOTSTRAP_WINDOWS_CASE", case)
        .env(
            "ROZI_TEST_TRANSPORT_TRACE",
            root.path().join("transport.log"),
        )
        .env("USERPROFILE", &home)
        .env("HOME", &home)
        .env("LOCALAPPDATA", &data_home)
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
    } else if !matches!(case, "existing" | "slow_probe") {
        child.env("ROZI_REMOTE_BINARY", env!("CARGO_BIN_EXE_rozi"));
    }
    let output = child.output().unwrap();
    let trace = std::fs::read_to_string(root.path().join("transport.log")).unwrap_or_default();
    if matches!(case, "slow_probe" | "slow_finalize") {
        assert!(
            trace.contains("simulating 50-second remote startup"),
            "delay was not exercised: {trace}"
        );
    }
    assert!(
        output.status.success(),
        "{}\n{}\ntransport trace:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        trace
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
fn windows_probe_allows_a_slow_remote_startup() {
    run_case("slow_probe");
}

#[test]
fn windows_finalize_allows_a_slow_remote_startup() {
    run_case("slow_finalize");
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
        // The slow cases shrink the connection timeout so a 50-second remote startup exceeds the
        // short command budget and fits only the PowerShell allowance. The rest use the default.
        connection_timeout_secs: if case.starts_with("slow_") {
            1
        } else {
            RemoteConfig::default().connection_timeout_secs
        },
        ..RemoteConfig::default()
    };
    let home = PathBuf::from(std::env::var_os("USERPROFILE").unwrap());
    let data_home = PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap());
    let final_path = managed(&data_home);

    match case.as_str() {
        "upload" | "slow_finalize" => {
            let path = ensure_remote_binary(&target, &config, true).unwrap();
            assert_same_path(&path, &final_path);
            assert_eq!(
                std::fs::read(final_path).unwrap(),
                std::fs::read(env!("CARGO_BIN_EXE_rozi")).unwrap()
            );
        }
        "existing" | "slow_probe" => {
            let path = ensure_remote_binary(&target, &config, true).unwrap();
            assert_same_path(&path, &final_path);
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
