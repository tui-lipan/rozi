//! Embeds the notes of the release being built, for rozi's What's new dialog.
//!
//! The release workflow points `ROZI_RELEASE_NOTES` at the generated `release-notes.md`; a relative
//! path is resolved against the package root, so the same value works inside the Linux build
//! container. Every other build - `cargo install`, nightlies, local work - embeds nothing, and the
//! dialog and its command stay out of the way.

use std::path::PathBuf;
use std::{env, fs};

const VARIABLE: &str = "ROZI_RELEASE_NOTES";

fn main() {
    println!("cargo::rerun-if-env-changed={VARIABLE}");
    let notes = match env::var_os(VARIABLE).filter(|path| !path.is_empty()) {
        Some(path) => {
            let path = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join(path);
            println!("cargo::rerun-if-changed={}", path.display());
            // A release asked for notes; building one silently without them would ship a version
            // that never says what changed.
            fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{VARIABLE}={}: {error}", path.display()))
        }
        None => String::new(),
    };
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("release-notes.md");
    fs::write(&out, notes).unwrap_or_else(|error| panic!("{}: {error}", out.display()));
}
