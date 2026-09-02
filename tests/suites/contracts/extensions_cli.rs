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
    rozi_in(temp, args, None)
}

fn rozi_in(temp: &tempfile::TempDir, args: &[&str], cwd: Option<&Path>) -> Output {
    rozi_in_with_env(temp, args, cwd, &[])
}

fn rozi_in_with_env(
    temp: &tempfile::TempDir,
    args: &[&str],
    cwd: Option<&Path>,
    env: &[(&str, &str)],
) -> Output {
    let config = temp.path().join("config/rozi/config.toml");
    std::fs::create_dir_all(config.parent().unwrap()).unwrap();
    if !config.exists() {
        std::fs::write(&config, "[extensions]\ndisabled = [\"disabled\"]\n").unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_rozi"));
    command
        .args(args)
        .env("XDG_DATA_HOME", temp.path().join("data"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("XDG_STATE_HOME", temp.path().join("state"))
        .env("XDG_CACHE_HOME", temp.path().join("cache"))
        .env("XDG_RUNTIME_DIR", temp.path().join("run"))
        .env("APPDATA", temp.path().join("config"))
        .env("LOCALAPPDATA", temp.path().join("data"))
        .env("ROZI_CONFIG", config);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.envs(env.iter().copied());
    command.output().unwrap()
}

fn write_source_extension(path: &Path, id: &str) {
    std::fs::create_dir_all(path).unwrap();
    std::fs::write(
        path.join("extension.toml"),
        format!(
            "[extension]\nid = \"{id}\"\ntitle = \"{id} title\"\nversion = \"1.0.0\"\napi = 1\n\
             [[commands]]\nid = \"open\"\nsend = \"echo {id}\\n\"\n"
        ),
    )
    .unwrap();
}

#[test]
fn local_install_copies_enables_and_removes_without_touching_the_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source extension");
    write_source_extension(&source, "disabled");

    let installed = rozi(&temp, &["extensions", "install", source.to_str().unwrap()]);
    assert!(
        installed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    let destination = temp.path().join("data/rozi/extensions/disabled");
    assert!(destination.join("extension.toml").is_file());
    assert!(
        !std::fs::read_to_string(temp.path().join("config/rozi/config.toml"))
            .unwrap()
            .contains("disabled =")
    );

    std::fs::write(source.join("source-only"), "source").unwrap();
    assert!(!destination.join("source-only").exists());
    let conflict = rozi(&temp, &["extensions", "install", source.to_str().unwrap()]);
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("already installed"));

    let removed = rozi(&temp, &["extensions", "remove", "disabled"]);
    assert!(
        removed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&removed.stdout),
        String::from_utf8_lossy(&removed.stderr)
    );
    assert!(!destination.exists());
    assert!(source.join("source-only").is_file());
    assert!(
        !temp
            .path()
            .join("data/rozi/extensions/.rozi/installations/disabled.toml")
            .exists()
    );
}

#[cfg(unix)]
#[test]
fn linked_install_reflects_edits_and_remove_only_unlinks_it() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("linked source");
    write_source_extension(&source, "linked");

    let installed = rozi(
        &temp,
        &["extensions", "install", "--link", source.to_str().unwrap()],
    );
    assert!(
        installed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    let destination = temp.path().join("data/rozi/extensions/linked");
    assert!(
        std::fs::symlink_metadata(&destination)
            .unwrap()
            .file_type()
            .is_symlink()
    );

    std::fs::write(source.join("live-edit"), "changed").unwrap();
    assert_eq!(
        std::fs::read_to_string(destination.join("live-edit")).unwrap(),
        "changed"
    );
    let removed = rozi(&temp, &["extensions", "remove", "linked"]);
    assert!(removed.status.success());
    assert!(!destination.exists());
    assert_eq!(
        std::fs::read_to_string(source.join("live-edit")).unwrap(),
        "changed"
    );
}

#[test]
fn invalid_sources_and_manifests_are_rejected_before_installation() {
    let temp = tempfile::tempdir().unwrap();
    let missing = rozi(
        &temp,
        &["extensions", "install", "./does-not-exist-or-look-like-git"],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("Invalid extension source"));

    let invalid = temp.path().join("invalid");
    std::fs::create_dir(&invalid).unwrap();
    std::fs::write(
        invalid.join("extension.toml"),
        "[extension]\nid = \"Uppercase\"\napi = 1\n",
    )
    .unwrap();
    let rejected = rozi(&temp, &["extensions", "install", invalid.to_str().unwrap()]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("not installable"));
    assert!(!temp.path().join("data/rozi/extensions/Uppercase").exists());
}

#[cfg(unix)]
#[test]
fn install_rejects_a_symlinked_rozi_data_destination() {
    use std::os::unix::fs::symlink;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("safe source");
    write_source_extension(&source, "safe-source");
    let redirected = temp.path().join("redirected");
    std::fs::create_dir_all(temp.path().join("data")).unwrap();
    std::fs::create_dir(&redirected).unwrap();
    symlink(&redirected, temp.path().join("data/rozi")).unwrap();

    let rejected = rozi(&temp, &["extensions", "install", source.to_str().unwrap()]);
    assert!(!rejected.status.success());
    assert!(String::from_utf8_lossy(&rejected.stderr).contains("not a directory"));
    assert!(!redirected.join("extensions").exists());
}

#[test]
fn git_install_records_the_original_remote_and_exact_revision() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("git source");
    write_source_extension(&source, "git-source");
    let run_git = |args: &[&str]| {
        let output = Command::new("git")
            .args(args)
            .current_dir(&source)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?}: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
        output
    };
    run_git(&["init"]);
    run_git(&["add", "."]);
    run_git(&[
        "-c",
        "user.name=Rozi Test",
        "-c",
        "user.email=rozi@example.invalid",
        "commit",
        "-m",
        "fixture",
    ]);
    let revision = String::from_utf8(run_git(&["rev-parse", "HEAD"]).stdout).unwrap();
    let revision = revision.trim();

    let remote = "https://example.invalid/git-source.git";
    let local_url = url::Url::from_directory_path(&source).unwrap().to_string();
    let rewrite_key = format!("url.{local_url}.insteadOf");
    let installed = rozi_in_with_env(
        &temp,
        &["extensions", "install", remote],
        None,
        &[
            ("GIT_CONFIG_COUNT", "1"),
            ("GIT_CONFIG_KEY_0", &rewrite_key),
            ("GIT_CONFIG_VALUE_0", remote),
        ],
    );
    assert!(
        installed.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    let destination = temp.path().join("data/rozi/extensions/git-source");
    assert!(destination.join(".git").is_dir());
    let metadata = std::fs::read_to_string(
        temp.path()
            .join("data/rozi/extensions/.rozi/installations/git-source.toml"),
    )
    .unwrap();
    let metadata: toml::Value = toml::from_str(&metadata).unwrap();
    assert_eq!(metadata["source"]["kind"].as_str(), Some("git"));
    assert_eq!(metadata["source"]["remote"].as_str(), Some(remote));
    assert_eq!(metadata["source"]["revision"].as_str(), Some(revision));
}

#[test]
fn list_extensions_reports_loaded_disabled_and_incompatible_in_all_formats() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("data");
    let loaded = write_extension(&root, "renamed-checkout", "loaded", 1);
    write_extension(&root, "disabled-checkout", "disabled", 1);
    write_extension(&root, "future-checkout", "future", 2);

    let text = rozi(&temp, &["extensions", "list"]);
    assert!(
        text.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&text.stdout),
        String::from_utf8_lossy(&text.stderr)
    );
    let stdout = String::from_utf8(text.stdout).unwrap();
    assert!(stdout.contains("NAME      TITLE           VERSION  COMMANDS  SERVICES  STATUS"));
    assert!(stdout.contains("loaded    loaded title    1.0.0    1         0         loaded"));
    assert!(stdout.contains("disabled  disabled title  1.0.0    1         0         disabled"));
    assert!(stdout.contains("requires extension API 2"));

    let verbose = rozi(&temp, &["extensions", "list", "--verbose"]);
    assert!(verbose.status.success());
    let stdout = String::from_utf8(verbose.stdout).unwrap();
    let loaded = std::fs::canonicalize(loaded).unwrap();
    assert!(stdout.lines().any(|line| {
        line.strip_prefix("  directory ")
            .and_then(|path| std::fs::canonicalize(path).ok())
            .is_some_and(|path| path == loaded)
    }));
    assert!(stdout.contains("loaded.open"));
    assert!(!stdout.contains("command.loaded.open"));

    let json = rozi(&temp, &["extensions", "list", "--json"]);
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

    let success = rozi(&temp, &["extensions", "check", good.to_str().unwrap()]);
    assert!(
        success.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&success.stdout),
        String::from_utf8_lossy(&success.stderr)
    );
    assert!(
        String::from_utf8(success.stdout)
            .unwrap()
            .contains("Status  loaded")
    );

    let failure = rozi(
        &temp,
        &["extensions", "check", future.to_str().unwrap(), "--json"],
    );
    assert!(!failure.status.success());
    let value: serde_json::Value = serde_json::from_slice(&failure.stdout).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["extension"]["status"], "incompatible");
    assert_eq!(value["extension"]["api"], 2);
    assert!(String::from_utf8(failure.stderr).unwrap().is_empty());
}

#[test]
fn new_extension_creates_a_valid_non_destructive_scaffold_in_unicode_paths() {
    let temp = tempfile::tempdir().unwrap();
    let parent = temp.path().join("author space 東京");
    std::fs::create_dir(&parent).unwrap();

    let created = rozi_in(&temp, &["extensions", "new", "sample-tools"], Some(&parent));
    assert!(
        created.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&created.stdout),
        String::from_utf8_lossy(&created.stderr)
    );
    let extension = parent.join("sample-tools");
    let manifest = std::fs::read_to_string(extension.join("extension.toml")).unwrap();
    let value: toml::Value = toml::from_str(&manifest).unwrap();
    assert_eq!(
        value["extension"]["api"].as_integer(),
        Some(i64::from(rozi::config::EXTENSION_API_VERSION))
    );
    assert!(extension.join("bin/hello.py").is_file());
    assert!(extension.join("README.md").is_file());

    let checked = rozi(
        &temp,
        &["extensions", "check", extension.to_str().unwrap(), "--json"],
    );
    assert!(
        checked.status.success(),
        "stdout: {}\nstderr: {}",
        String::from_utf8_lossy(&checked.stdout),
        String::from_utf8_lossy(&checked.stderr)
    );
    let diagnostic: serde_json::Value = serde_json::from_slice(&checked.stdout).unwrap();
    assert_eq!(diagnostic["extension"]["commands"][0], "sample-tools.hello");

    let repeated = rozi_in(&temp, &["extensions", "new", "sample-tools"], Some(&parent));
    assert!(!repeated.status.success());
    assert!(String::from_utf8_lossy(&repeated.stderr).contains("destination already exists"));
    assert_eq!(
        std::fs::read_to_string(extension.join("extension.toml")).unwrap(),
        manifest
    );

    for id in ["Uppercase", "two.words", "../escape", "rozi"] {
        let rejected = rozi_in(&temp, &["extensions", "new", id], Some(&parent));
        assert!(!rejected.status.success(), "accepted invalid id {id:?}");
        assert!(!parent.join(id).exists());
    }
}

#[test]
fn check_extension_explains_launch_cwd_and_safe_environment_details() {
    let temp = tempfile::tempdir().unwrap();
    let extension = temp.path().join("diagnostic extension");
    std::fs::create_dir_all(extension.join("bin")).unwrap();
    std::fs::write(extension.join("bin/tool.py"), "print('ok')\n").unwrap();
    std::fs::write(
        extension.join("extension.toml"),
        "[extension]\nid = \"diagnostic\"\napi = 1\n\
         [[navigation_targets]]\nname = \"vim\"\nprograms = [\"vim\", \"nvim\"]\n\
         [[commands]]\nid = \"open\"\nexec = [\"python\", \"{extension_dir}/bin/tool.py\", \"arg with space\"]\n\
         [[services]]\nname = \"watch\"\nshell = \"echo ready\"\ncwd = \".\"\nrestart = \"never\"\n\
         [services.env]\nTOKEN = \"do-not-print\"\n",
    )
    .unwrap();

    let output = rozi(&temp, &["extensions", "check", extension.to_str().unwrap()]);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains(r#"launch: ["python","#));
    assert!(stdout.contains("arg with space"));
    assert!(stdout.contains("cwd: focused-pane"));
    assert!(stdout.contains("ROZI_EXTENSION=diagnostic"));
    assert!(stdout.contains("ROZI_EXTENSION_GENERATION=<assigned-at-load>"));
    assert!(stdout.contains("NAVIGATION TARGETS"));
    assert!(stdout.contains("vim  vim, nvim"));
    assert!(stdout.contains("manifest env: TOKEN (values redacted)"));
    assert!(!stdout.contains("do-not-print"));

    let json = rozi(
        &temp,
        &["extensions", "check", extension.to_str().unwrap(), "--json"],
    );
    assert!(json.status.success());
    let document: serde_json::Value = serde_json::from_slice(&json.stdout).unwrap();
    assert_eq!(
        document["extension"]["command_details"][0]["launch"]["kind"],
        "direct"
    );
    assert_eq!(
        document["extension"]["service_details"][0]["configured_env_keys"][0],
        "TOKEN"
    );
    assert_eq!(
        document["extension"]["navigation_targets"][0]["programs"],
        serde_json::json!(["vim", "nvim"])
    );
    assert!(
        !String::from_utf8(json.stdout)
            .unwrap()
            .contains("do-not-print")
    );
}
