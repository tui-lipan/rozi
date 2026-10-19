//! The actual CLI must retain --config through its control-command fast path.
mod common;

use rozi::control::{ControlCommand, ControlRequest};
use rozi::platform::paths::PlatformEnv;
use rozi::session::headless::run_session_control;
use rozi::session::server::ServerSettings;

#[test]
fn split_uses_explicit_config_before_cwd_fallback_and_preserves_cwd_override() {
    rozi::test_support::isolate_user_dirs();
    let env = PlatformEnv::from_process();
    let root = tempfile::tempdir().unwrap();
    let root = root.path().canonicalize().unwrap();
    let configured = root.join("configured directory");
    let explicit = root.join("explicit directory");
    std::fs::create_dir(&configured).unwrap();
    std::fs::create_dir(&explicit).unwrap();
    std::fs::write(
        root.join("custom.toml"),
        format!(
            "cwd = {}\n",
            toml::Value::String(configured.to_string_lossy().into_owned())
        ),
    )
    .unwrap();
    std::fs::write(root.join("ambient.toml"), "cwd = \".\"\n").unwrap();
    let server = common::spawn_listener(ServerSettings::default());

    for override_cwd in [false, true] {
        let mut cli = std::process::Command::new(env!("CARGO_BIN_EXE_rozi"));
        cli.current_dir(&root)
            .args([
                "--config",
                "custom.toml",
                "--session",
                server.session(),
                "split",
            ])
            .env("ROZI_CONFIG", root.join("ambient.toml"))
            .env("HOME", env.home.as_ref().unwrap())
            .env("XDG_CONFIG_HOME", env.xdg_config_home.as_ref().unwrap())
            .env("XDG_STATE_HOME", env.xdg_state_home.as_ref().unwrap())
            .env("XDG_CACHE_HOME", env.xdg_cache_home.as_ref().unwrap())
            .env("XDG_DATA_HOME", env.xdg_data_home.as_ref().unwrap())
            .env("XDG_RUNTIME_DIR", env.xdg_runtime_dir.as_ref().unwrap())
            .env("APPDATA", env.appdata.as_ref().unwrap())
            .env("LOCALAPPDATA", env.local_appdata.as_ref().unwrap())
            .env_remove("ROZI_SOCKET")
            .env_remove("ROZI_PANE")
            .env_remove("ROZI_SESSION_INSTANCE")
            .env_remove("ROZI_EXTENSION")
            .env_remove("ROZI_EXTENSION_CREDENTIAL");
        if override_cwd {
            cli.args(["--cwd", "explicit directory"]);
        }
        let output = cli.output().unwrap();
        assert!(
            output.status.success(),
            "stdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let spawned: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(spawned["data"]["pty_ready"], true);
        let response = run_session_control(
            server.session(),
            ControlRequest {
                command: ControlCommand::ListPanes,
                source_pane: None,
                source_session: None,
                extension: None,
                credential: None,
            },
        )
        .unwrap();
        assert!(response.ok, "{response:?}");
        let panes = response.data.unwrap();
        let pane = panes
            .as_array()
            .unwrap()
            .iter()
            .find(|pane| pane["id"] == spawned["data"]["id"])
            .unwrap();
        let expected = if override_cwd { &explicit } else { &configured };
        assert_eq!(pane["cwd"], expected.to_string_lossy().as_ref());
    }
}
