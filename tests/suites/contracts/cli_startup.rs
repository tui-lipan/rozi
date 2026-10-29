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

    /// `sessions new --detach` starts a server that scripts can drive with no client attached,
    /// refuses a name that is already running, and `sessions kill` removes it again.
    #[test]
    fn detached_session_starts_empty_and_takes_panes_without_a_client() {
        let root = isolated_home("new-detach");
        let rozi = |args: &[&str]| command(&root).args(args).output().unwrap();
        let stderr =
            |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();

        let created = rozi(&["sessions", "new", "scratch", "--detach"]);
        assert!(created.status.success(), "create: {}", stderr(&created));
        let again = rozi(&["sessions", "new", "scratch", "--detach"]);
        assert!(!again.status.success());
        assert!(
            stderr(&again).contains("Session `scratch` is already running"),
            "duplicate diagnostic: {}",
            stderr(&again)
        );

        let panes = rozi(&["--session", "scratch", "list-panes", "--format", "json"]);
        assert_eq!(
            String::from_utf8_lossy(&panes.stdout).trim(),
            r#"{"data":[],"ok":true}"#
        );
        let split = rozi(&[
            "--session",
            "scratch",
            "split",
            "--argv",
            "sh",
            "-c",
            "echo detached-ready; sleep 30",
        ]);
        assert!(split.status.success(), "split: {}", stderr(&split));
        let captured = rozi(&[
            "--session",
            "scratch",
            "capture-pane",
            "--target",
            "1",
            "--wait-for",
            "detached-ready",
            "--timeout",
            "10s",
        ]);
        assert!(captured.status.success(), "capture: {}", stderr(&captured));

        let killed = rozi(&["sessions", "kill", "scratch"]);
        assert!(killed.status.success(), "kill: {}", stderr(&killed));
        let listed = rozi(&["sessions", "list", "--format", "json"]);
        assert_eq!(String::from_utf8_lossy(&listed.stdout).trim(), "[]");
        fs::remove_dir_all(root).unwrap();
    }

    /// A detached session stopped without `sessions kill` — a shutdown or crash — stays restorable
    /// like any named session, unless it was created with `--no-resurrect`.
    #[test]
    fn no_resurrect_detached_session_leaves_nothing_restorable_after_a_stop() {
        // Short label and names: the session socket must stay under macOS's 104-byte `sun_path`.
        let root = isolated_home("nores");
        let rozi = |args: &[&str]| command(&root).args(args).output().unwrap();
        let stderr =
            |output: &std::process::Output| String::from_utf8_lossy(&output.stderr).into_owned();
        for (name, extra, restorable) in [
            ("keep", None, true),
            ("drop", Some("--no-resurrect"), false),
        ] {
            let mut create = vec!["sessions", "new", name, "--detach"];
            create.extend(extra);
            let created = rozi(&create);
            assert!(
                created.status.success(),
                "{name}: create: {}",
                stderr(&created)
            );
            let split = rozi(&["--session", name, "split", "--argv", "sh", "-c", "sleep 30"]);
            assert!(split.status.success(), "{name}: split: {}", stderr(&split));

            // What a machine shutdown does to the server: a stop signal, not `sessions kill`. The
            // PID comes from this test's own session socket, so no other rozi on the machine is
            // ever signalled.
            let socket = root.join(format!("runtime/rozi/session-{name}.sock"));
            let server_pid = rozi::platform::ipc::IpcEndpoint::at_path(&socket)
                .connect()
                .unwrap_or_else(|err| panic!("{name}: connect {}: {err}", socket.display()))
                .peer_pid()
                .expect("Linux and macOS report a socket peer's PID");
            let signalled = Command::new("kill")
                .args(["-TERM", &server_pid.to_string()])
                .status()
                .unwrap();
            assert!(signalled.success(), "{name}: kill -TERM {server_pid}");
            let deadline = Instant::now() + Duration::from_secs(5);
            let listed = loop {
                let listed = rozi(&["sessions", "list", "--format", "json"]);
                let listed = String::from_utf8_lossy(&listed.stdout).into_owned();
                let stopped = !listed.contains(&format!(r#""name": "{name}""#))
                    || listed.contains(r#""status": "restorable""#);
                if stopped || Instant::now() >= deadline {
                    break listed;
                }
                std::thread::sleep(Duration::from_millis(50));
            };
            assert_eq!(
                listed.contains(&format!(r#""name": "{name}""#)),
                restorable,
                "{name}: {listed}"
            );
            let _ = rozi(&["sessions", "kill", name]);
        }
        fs::remove_dir_all(root).unwrap();
    }

    /// Racing creators of one name: exactly one is told it created the session, and every other one
    /// is refused, whatever the platform can say about a socket peer.
    #[test]
    fn concurrent_detached_creates_of_one_name_have_exactly_one_winner() {
        let root = isolated_home("new-detach-race");
        let racers: Vec<_> = (0..6)
            .map(|_| {
                command(&root)
                    .args(["sessions", "new", "race", "--detach"])
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .unwrap()
            })
            .collect();
        let outputs: Vec<_> = racers
            .into_iter()
            .map(|racer| racer.wait_with_output().unwrap())
            .collect();
        let winners = outputs
            .iter()
            .filter(|output| output.status.success())
            .count();
        for output in outputs.iter().filter(|output| !output.status.success()) {
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                stderr.contains("Session `race` is already running"),
                "loser diagnostic: {stderr}"
            );
        }
        assert_eq!(winners, 1, "every racer's result: {outputs:?}");

        let listed = command(&root)
            .args(["sessions", "list", "--format", "json"])
            .output()
            .unwrap();
        let listed = String::from_utf8_lossy(&listed.stdout);
        assert_eq!(listed.matches(r#""name": "race""#).count(), 1, "{listed}");
        let killed = command(&root)
            .args(["sessions", "kill", "race"])
            .output()
            .unwrap();
        assert!(killed.status.success());
        // A refused racer stops the server it spawned, so none can take the name afterwards.
        std::thread::sleep(Duration::from_millis(200));
        let after = command(&root)
            .args(["sessions", "list", "--format", "json"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&after.stdout).trim(), "[]");
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

        // The listener polls, but this read should block until the client's line arrives. Linux
        // hands `accept` a blocking socket regardless of the listener; the BSDs, macOS included,
        // let it inherit `O_NONBLOCK`, so the read returned `EAGAIN` for whatever had not arrived
        // in the same instant - a failure that depended on scheduling, not on the behaviour here.
        stream.set_nonblocking(false).unwrap();

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
