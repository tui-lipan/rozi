//! Embeds release notes for rozi's What's new dialog: the release being built, and the releases
//! before it.
//!
//! The release workflow points `ROZI_RELEASE_NOTES` at the generated `release-notes.md`, and
//! `ROZI_EARLIER_RELEASE_NOTES` at a directory of earlier releases' published notes, one
//! `vX.Y.Z.md` per release. A relative path is resolved against the package root, so the same value
//! works inside the Linux build container. Every other build - `cargo install`, nightlies, local
//! work - embeds nothing, and the dialog and its command stay out of the way.

use std::path::{Path, PathBuf};
use std::{env, fs};

const VARIABLE: &str = "ROZI_RELEASE_NOTES";
const EARLIER_VARIABLE: &str = "ROZI_EARLIER_RELEASE_NOTES";

fn main() {
    println!("cargo::rerun-if-env-changed={VARIABLE}");
    println!("cargo::rerun-if-env-changed={EARLIER_VARIABLE}");
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let notes = match requested_path(VARIABLE) {
        // A release asked for notes; building one silently without them would ship a version
        // that never says what changed.
        Some(path) => fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{VARIABLE}={}: {error}", path.display())),
        None => String::new(),
    };
    write(&out.join("release-notes.md"), &notes);
    write(
        &out.join("earlier-release-notes.rs"),
        &earlier_table(&out.join("earlier-release-notes")),
    );
}

/// The file or directory `variable` names, resolved against the package root, or `None` when the
/// variable is unset or empty.
fn requested_path(variable: &str) -> Option<PathBuf> {
    let path = env::var_os(variable).filter(|path| !path.is_empty())?;
    let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(path);
    println!("cargo::rerun-if-changed={}", path.display());
    Some(path)
}

/// Copy every `vX.Y.Z.md` the earlier-notes directory holds into `copies`, and return Rust source
/// for a `(version, notes)` table that embeds the copies.
fn earlier_table(copies: &Path) -> String {
    let mut table = String::from("&[\n");
    let Some(dir) = requested_path(EARLIER_VARIABLE) else {
        return table + "]\n";
    };
    let entries = fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{EARLIER_VARIABLE}={}: {error}", dir.display()));
    let mut releases = entries
        .map(|entry| {
            let path = entry
                .unwrap_or_else(|error| panic!("{EARLIER_VARIABLE}={}: {error}", dir.display()))
                .path();
            // Anything else in the directory is a workflow mistake, not something to skip quietly.
            let version = path
                .file_name()
                .and_then(|name| name.to_str())
                .and_then(|name| name.strip_prefix('v')?.strip_suffix(".md"))
                .filter(|version| is_plain_version(version))
                .unwrap_or_else(|| panic!("{}: not a vX.Y.Z.md release note", path.display()))
                .to_string();
            (version, path)
        })
        .collect::<Vec<_>>();
    releases.sort();
    fs::create_dir_all(copies).unwrap_or_else(|error| panic!("{}: {error}", copies.display()));
    for (version, path) in releases {
        let notes =
            fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
        write(&copies.join(format!("v{version}.md")), &notes);
        table.push_str(&format!(
            "    (\"{version}\", include_str!(concat!(env!(\"OUT_DIR\"), \"/earlier-release-notes/v{version}.md\"))),\n"
        ));
    }
    table + "]\n"
}

fn is_plain_version(version: &str) -> bool {
    let parts = version.split('.').collect::<Vec<_>>();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn write(path: &Path, contents: &str) {
    fs::write(path, contents).unwrap_or_else(|error| panic!("{}: {error}", path.display()));
}
