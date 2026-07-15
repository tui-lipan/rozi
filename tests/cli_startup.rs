#[cfg(unix)]
mod unix {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn isolated_home(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "hyprmux-cli-{label}-{}-{nonce}",
            std::process::id()
        ));
        for dir in ["config", "state", "runtime"] {
            let path = root.join(dir);
            fs::create_dir_all(&path).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        root
    }

    fn command(root: &std::path::Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_hyprmux"));
        command
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
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
        assert!(stderr.contains("Create it with: hyprmux new wok"));
        let runtime_entries = fs::read_dir(root.join("runtime/hyprmux")).unwrap().count();
        assert_eq!(runtime_entries, 0, "startup error must not launch a server");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn attach_only_missing_session_suggests_canonical_profile() {
        let root = isolated_home("attach");
        let profiles = root.join("config/hyprmux/profiles");
        fs::create_dir_all(&profiles).unwrap();
        fs::write(profiles.join("work.toml"), "version = 1\n").unwrap();

        let output = command(&root).args(["attach", "work"]).output().unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("Session `work` is not running."));
        assert!(stderr.contains("Start it with: hyprmux work"));
        fs::remove_dir_all(root).unwrap();
    }
}
