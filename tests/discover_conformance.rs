//! The same corpus can be checked against the Python index with scripts/check_discover_conformance.py.
use std::process::Command;

#[test]
fn extensions_check_matches_discover_eligibility_corpus() {
    let cases: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/discover-eligibility.json")).unwrap();
    for case in cases.as_array().unwrap() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join(".git")).unwrap();
        // Repository files the manifest refers to, as path to size in bytes.
        for (path, size) in case["files"].as_object().into_iter().flatten() {
            let path = temp.path().join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            let file = std::fs::File::create(path).unwrap();
            file.set_len(size.as_u64().unwrap()).unwrap();
        }
        std::fs::write(
            temp.path().join("extension.toml"),
            case["manifest"].as_str().unwrap(),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_rozi"))
            .args(["extensions", "check"])
            .arg(temp.path())
            .arg("--json")
            .env("ROZI_CONFIG", temp.path().join("config.toml"))
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("XDG_STATE_HOME", temp.path().join("state"))
            .env("XDG_CACHE_HOME", temp.path().join("cache"))
            .env("APPDATA", temp.path().join("appdata"))
            .env("LOCALAPPDATA", temp.path().join("localappdata"))
            .output()
            .unwrap();
        let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(
            report["discover"]["ready"], case["eligible"],
            "{}: {report}",
            case["name"]
        );
        if let Some(valid) = case["install_valid"].as_bool() {
            assert_eq!(output.status.success(), valid, "{}: {report}", case["name"]);
        }
    }
}
