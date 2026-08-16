use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn write_extension(root: &Path, directory: &str, id: &str, api: u32) -> PathBuf {
    let path = root.join("rozi/extensions").join(directory);
    std::fs::create_dir_all(&path).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for private in [root.join("rozi"), root.join("rozi/extensions")] {
            std::fs::set_permissions(private, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
    }
    std::fs::write(
        path.join("extension.toml"),
        format!(
            "[extension]\nid = \"{id}\"\ntitle = \"{id} title\"\nversion = \"1.0.0\"\napi = {api}\n\
             [[commands]]\nid = \"open\"\nsend = \"echo {id}\\n\"\n"
        ),
    )
    .unwrap();
    path
}

fn rozi(temp: &tempfile::TempDir, args: &[&str]) -> Output {
    let config = temp.path().join("config/rozi/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    if !config.exists() {
        std::fs::write(&config, "[extensions]\ndisabled = [\"disabled\"]\n").unwrap();
    }
    Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(args)
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_RUNTIME_DIR", temp.path().join("run"))
        .env("ROZI_CONFIG", config)
        .output()
        .unwrap()
}

#[test]
fn list_extensions_reports_loaded_disabled_and_incompatible_in_all_formats() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let loaded = write_extension(&root, "renamed-checkout", "loaded", 1);
    write_extension(&root, "disabled-checkout", "disabled", 1);
    write_extension(&root, "future-checkout", "future", 2);

    let text = rozi(&temp, &["list-extensions"]);
    assert!(
        text.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("NAME\tTITLE\tVERSION\tCOMMANDS\tSERVICES\tSTATUS"));
    assert!(stdout.contains("loaded\tloaded title\t1.0.0\t1\t0\tloaded"));
    assert!(stdout.contains("disabled\tdisabled title\t1.0.0\t1\t0\tdisabled"));
    assert!(stdout.contains("requires extension API 2"));

    let verbose = rozi(&temp, &["list-extensions", "--verbose"]);
    assert!(verbose.status.success());
    let stdout = String::from_utf8(verbose.stdout).unwrap();
    assert!(stdout.contains(&loaded.display().to_string()));
    assert!(stdout.contains("loaded.open"));
    assert!(!stdout.contains("command.loaded.open"));

    let json = rozi(&temp, &["list-extensions", "--json"]);
    assert!(json.status.success());
    let document: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(document["schema_version"], 1);
    let loaded = document["extensions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["id"] == "loaded")
        .unwrap();
    assert_eq!(loaded["status"], "loaded");
    assert_eq!(loaded["commands"][0], "loaded.open");
    assert!(loaded["errors"].as_array().unwrap().is_empty());
}

#[test]
fn check_extension_has_deterministic_success_and_failure_exit_codes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let good = write_extension(&root, "good-checkout", "good", 1);
    let future = write_extension(&root, "future-checkout", "future", 2);

    let success = rozi(&temp, &["check-extension", good.to_str().unwrap()]);
    assert!(
        success.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&success.stdout),
        String::from_utf8_lossy(&success.stderr)
    );
    assert!(
        String::from_utf8(success.stdout)
            .unwrap()
            .contains("✓ manifest valid")
    );

    let failure = rozi(
        &temp,
        &["check-extension", future.to_str().unwrap(), "--json"],
    );
    assert!(!failure.status.success());
    let value: serde_json::Value = serde_json::from_slice(&failure.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["extension"]["status"], "incompatible");
    assert_eq!(value["extension"]["api"], 2);
    assert!(String::from_utf8(failure.stderr).unwrap().is_empty());
}
