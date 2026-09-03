#[cfg(unix)]
mod unix {
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixListener;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    /// A throwaway `$HOME` for one CLI invocation.
    ///
    /// Rooted through [`rozi::test_support::socket_safe_temp_dir`] because
    /// `control_clients_skip_managed_installation_recovery` binds a socket inside what this
    /// returns: under macOS's per-user `TMPDIR` the path is past `sun_path` before the test starts.
    fn isolated_home(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = rozi::test_support::socket_safe_temp_dir()
            .join(format!("rozi-cli-{label}-{}-{nonce}", std::process::id()));
        for dir in ["config", "state", "data", "runtime"] {
            let path = root.join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    fn command(root: &std::path::Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
        command
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"));
        command
    }

    #[test]
    fn unknown_bare_target_errors_before_starting_a_server() {
        let root = isolated_home("unknown");
        let output = command(&root).arg("wok").output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("No session or profile named `wok`."));
        assert!(stderr.contains("Create it with: rozi sessions new wok"));
        let runtime_path = root.join("runtime/rozi");
        let runtime_entries = fs::read_dir(&runtime_path)
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(runtime_entries, 0, "startup error must not launch a server");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attach_only_missing_session_suggests_canonical_profile() {
        let root = isolated_home("attach");
        let profiles = root.join("config/rozi/profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("work.toml"), "version = 1\n").unwrap();

        let output = command(&root)
            .args(["sessions", "attach", "work"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Session `work` is not running."));
        assert!(stderr.contains("Start it with: rozi work"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn killing_a_missing_session_fails_with_a_diagnostic() {
        let root = isolated_home("kill-missing");
        let output = command(&root)
            .args(["sessions", "kill", "missing"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("Session `missing` was not found."),
            "missing session diagnostic: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// Printing help must not create the runtime directory it names.
    ///
    /// `endpoint_help()` resolved the path by calling the constructor, which creates and validates
    /// it. On Windows the runtime directory is `%LOCALAPPDATA%\rozi\run`, so that also created
    /// `%LOCALAPPDATA%\rozi` - the managed installation root - with whatever permissions its parent
    /// hands down. `install.ps1` probes the payload with `--help` before running `install`, so the
    /// installer established the install root itself, unprotected, immediately before failing on it.
    #[test]
    fn printing_help_does_not_create_the_runtime_directory() {
        let root = isolated_home("help-no-side-effects");
        let runtime = root.join("runtime/rozi");
        for argument in ["--help", "--version"] {
            let output = command(&root).arg(argument).output().unwrap();
            assert!(output.status.success(), "{argument} failed");
            assert!(
                !runtime.exists(),
                "{argument} created the runtime directory: {}",
                runtime.display()
            );
        }
        // The path is still named in the advanced help; resolving it must be all that happens.
        let output = command(&root)
            .args(["--help", "--advanced"])
            .output()
            .unwrap();
        assert!(
            String::from_utf8_lossy(&output.stdout).contains(&runtime.display().to_string()),
            "advanced help stopped naming the runtime directory"
        );
        assert!(
            !runtime.exists(),
            "advanced help created the runtime directory"
        );
        fs::remove_dir_all(root).unwrap();
    }

    /// `--version` and `--help` must survive a managed layout that will not validate.
    ///
    /// A Windows CI runner caught the original: startup recovery rejected the configuration and
    /// aborted before either could print, so the two commands you would reach for to diagnose the
    /// installation were the two the installation could silence.
    #[test]
    fn version_and_help_survive_an_unusable_managed_layout() {
        let root = isolated_home("diagnostics");
        // A file where the managed data root must be a directory: recovery cannot read this layout
        // under any configuration.
        fs::write(root.join("data").join("rozi"), b"not a directory").unwrap();
        for argument in ["--version", "--help"] {
            let output = command(&root).arg(argument).output().unwrap();
            assert!(
                output.status.success(),
                "{argument} failed on a broken layout: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                !String::from_utf8_lossy(&output.stdout).trim().is_empty(),
                "{argument} printed nothing"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn control_clients_skip_managed_installation_recovery() {
        let root = isolated_home("control-client");
        fs::write(root.join("data").join("rozi"), b"not a directory").unwrap();

        let socket = root.join("control.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let mut child = command(&root)
            .args([
                "--socket",
                socket.to_str().unwrap(),
                "run-action",
                "focus-left",
            ])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let (mut stream, _) = loop {
            match listener.accept() {
                Ok(connection) => break connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    assert!(
                        child.try_wait().unwrap().is_none(),
                        "control client exited before connecting"
                    );
                    assert!(Instant::now() < deadline, "control client did not connect");
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(error) => panic!("control listener failed: {error}"),
            }
        };

        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap())
            .read_line(&mut request)
            .unwrap();
        assert!(request.contains(r#""cmd":"run-action""#), "{request}");
        assert!(request.contains(r#""action":"focus-left""#), "{request}");
        writeln!(stream, r#"{{"ok":true}}"#).unwrap();

        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "control client failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unmanaged_update_refuses_before_runtime_or_session_startup() {
        let root = isolated_home("unmanaged-update");
        // The refusal names whichever channel owns the binary, so pin the ones that are relocatable
        // to empty scratch directories. Without this the message would depend on where the
        // developer's checkout happens to live rather than on the behaviour under test.
        let output = command(&root)
            .env("CARGO_HOME", root.join("cargo"))
            .env("MISE_DATA_DIR", root.join("mise"))
            .env("HOMEBREW_PREFIX", root.join("brew"))
            .arg("update")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains("was not installed by its managed installer"),
            "unmanaged refusal should name the channel that owns the install: {stderr}"
        );
        let runtime_entries = fs::read_dir(root.join("runtime/rozi"))
            .map(|entries| entries.count())
            .unwrap_or(0);
        assert_eq!(
            runtime_entries, 0,
            "update refusal must not create endpoints"
        );
        assert!(
            !root.join("data/rozi").exists(),
            "unmanaged update must not create managed state"
        );
        fs::remove_dir_all(root).unwrap();
    }
}
