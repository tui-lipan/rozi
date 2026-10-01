use std::path::{Path, PathBuf};
use std::process::Command;

const EXAMPLES: &[(&str, &[&str], &[&str])] = &[
    (
        "git-tools",
        &["git-tools.branches", "git-tools.worktrees"],
        &[],
    ),
    (
        "pr-dashboard",
        &["pr-dashboard.open"],
        &["pr-dashboard.watch"],
    ),
    ("docker", &["docker.containers"], &[]),
    ("ssh-tools", &["ssh-tools.hosts"], &[]),
    (
        "agent-activity",
        &["agent-activity.open"],
        &["agent-activity.watch"],
    ),
];

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf()
}

fn check_extension(path: &Path) -> serde_json::Value {
    let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
        .args(["extensions", "check", path.to_str().unwrap(), "--json"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\nstdout: {}\nstderr: {}",
        path.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn canonical_extensions_conform_to_public_manifest_and_launch_contracts() {
    let root = repository_root().join("examples/extensions");
    for (id, expected_commands, expected_services) in EXAMPLES {
        let document = check_extension(&root.join(id));
        assert_eq!(document["schema_version"], 1);
        let extension = &document["extension"];
        assert_eq!(extension["id"], *id);
        assert_eq!(extension["api"], rozi::config::EXTENSION_API_VERSION);
        assert_eq!(extension["status"], "loaded");
        assert_eq!(
            extension["commands"],
            serde_json::json!(expected_commands),
            "{id}"
        );
        assert_eq!(
            extension["services"],
            serde_json::json!(expected_services),
            "{id}"
        );

        for command in extension["command_details"].as_array().unwrap() {
            assert_eq!(command["cwd"], "focused-pane", "{id}: {command}");
            assert_eq!(
                command["injected_env"]["ROZI_EXTENSION"], *id,
                "{id}: {command}"
            );
            assert_eq!(
                command["injected_env"]["ROZI_EXTENSION_GENERATION"], "<assigned-at-load>",
                "{id}: {command}"
            );
        }
        for service in extension["service_details"].as_array().unwrap() {
            assert!(
                service["cwd"].as_str().is_some_and(|cwd| !cwd.is_empty()),
                "{id}: {service}"
            );
            assert_eq!(
                service["injected_env"]["ROZI_EXTENSION"], *id,
                "{id}: {service}"
            );
            assert_eq!(
                service["injected_env"]["ROZI_EXTENSION_GENERATION"], "<assigned-at-load>",
                "{id}: {service}"
            );
        }
    }
}

#[test]
fn canonical_python_sources_parse_without_importing_rozi() {
    let python = Command::new("python").arg("--version").output();
    if !python.is_ok_and(|output| output.status.success()) {
        return;
    }

    let root = repository_root().join("examples/extensions");
    for (id, _, _) in EXAMPLES {
        let bin = root.join(id).join("bin");
        for entry in std::fs::read_dir(bin).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("py") {
                continue;
            }
            let source = std::fs::read_to_string(&path).unwrap();
            assert!(!source.contains("import rozi"), "{}", path.display());
            let script = "import ast, pathlib, sys; ast.parse(pathlib.Path(sys.argv[1]).read_text(encoding='utf-8'))";
            let output = Command::new("python")
                .args(["-c", script])
                .arg(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}: {}",
                path.display(),
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
}

#[test]
fn canonical_extension_logic_passes_without_live_optional_services() {
    let python = Command::new("python").arg("--version").output();
    if !python.is_ok_and(|output| output.status.success()) {
        return;
    }

    let root = repository_root().join("examples/extensions");
    for (id, _, _) in EXAMPLES {
        let tests = root.join(id).join("tests");
        let output = Command::new("python")
            .args(["-m", "unittest", "discover"])
            .arg(&tests)
            .args(["-p", "test_*.py"])
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{id}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

#[test]
fn extension_author_skill_tracks_the_current_public_contract() {
    let path = repository_root().join(".agents/skills/rozi-extension/SKILL.md");
    let skill = std::fs::read_to_string(path).unwrap();
    assert!(skill.lines().count() < 500);
    for required in [
        "api = 1",
        "rozi extensions new",
        "rozi extensions check",
        "ROZI_EXTENSION",
        "ROZI_EXTENSION_DIR",
        "ROZI_EXTENSION_GENERATION",
        "rozi pick --json",
        "rozi publish",
        "rozi subscribe",
        "rozi run-action",
        "trusted local executable code",
    ] {
        assert!(skill.contains(required), "skill omitted {required:?}");
    }
}
