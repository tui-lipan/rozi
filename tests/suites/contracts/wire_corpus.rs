//! The golden wire corpus: one JSON example of every session message variant.
//!
//! `tests/fixtures/wire/current/` describes this build: `client/<type>.json` and
//! `server/<type>.json`, one per variant. At each release the directory is copied to
//! `tests/fixtures/wire/<release>/` with a `manifest.json` naming its generation (see
//! `dev/release-process.md`). The current build has to decode every fixture of every release whose
//! generation is still in the support window, and every message it sends to a peer without
//! optional capabilities has to validate against the frozen schema of each generation in the
//! window. Together they check backward compatibility without old binaries.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use rozi::session::protocol::{
    ClientMessage, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION, ServerMessage,
};
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;

fn wire_fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire")
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

/// Each message direction, by fixture directory and schema definition.
const DIRECTIONS: [(&str, &str); 2] = [("client", "ClientMessage"), ("server", "ServerMessage")];

/// The variant tags a schema document lists for one message enum, without the catch-all.
fn schema_tags(schema: &Value, definition: &str) -> BTreeSet<String> {
    schema["$defs"][definition]["oneOf"]
        .as_array()
        .expect("message enums are unions")
        .iter()
        .filter_map(|variant| variant["properties"]["type"]["const"].as_str())
        .filter(|tag| *tag != "unknown")
        .map(str::to_owned)
        .collect()
}

fn fixture_tags(dir: &Path) -> BTreeSet<String> {
    std::fs::read_dir(dir)
        .unwrap_or_else(|err| panic!("read {}: {err}", dir.display()))
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            (path.extension()? == "json").then(|| path.file_stem()?.to_str().map(str::to_owned))?
        })
        .collect()
}

/// Every release directory whose generation is still in this build's window, with `current`.
fn releases_in_window() -> Vec<(String, PathBuf)> {
    let mut releases = Vec::new();
    for entry in std::fs::read_dir(wire_fixtures()).expect("wire fixtures") {
        let path = entry.expect("fixture entry").path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if !path.is_dir() || name == "schema" {
            continue;
        }
        if name != "current" {
            let generation = read_json(&path.join("manifest.json"))["generation"]
                .as_u64()
                .unwrap_or_else(|| panic!("{name}/manifest.json names no generation"))
                as u32;
            if !(MIN_SUPPORTED_PROTOCOL..=PROTOCOL_VERSION).contains(&generation) {
                continue;
            }
        }
        releases.push((name, path));
    }
    releases.sort();
    releases
}

/// Decode a fixture as this build would receive it, refusing the catch-all.
fn decode<T: DeserializeOwned + std::fmt::Debug + PartialEq>(
    path: &Path,
    unknown: &T,
) -> Result<T, String> {
    let text = std::fs::read_to_string(path).map_err(|err| err.to_string())?;
    let message: T = serde_json::from_str(&text).map_err(|err| err.to_string())?;
    if &message == unknown {
        return Err("decodes only as the unknown catch-all".into());
    }
    Ok(message)
}

fn each_fixture(dir: &Path, mut check: impl FnMut(&str, &Path) -> Result<(), String>) {
    let mut failures = Vec::new();
    for (direction, _) in DIRECTIONS {
        let dir = dir.join(direction);
        for tag in fixture_tags(&dir) {
            let path = dir.join(format!("{tag}.json"));
            if let Err(problem) = check(direction, &path) {
                failures.push(format!("{}: {problem}", path.display()));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn the_corpus_has_one_example_of_every_message_variant() {
    let schema =
        read_json(&Path::new(env!("CARGO_MANIFEST_DIR")).join("schemas/session-wire.schema.json"));
    for (direction, definition) in DIRECTIONS {
        let expected = schema_tags(&schema, definition);
        let present = fixture_tags(&wire_fixtures().join("current").join(direction));
        let missing: Vec<_> = expected.difference(&present).collect();
        let stale: Vec<_> = present.difference(&expected).collect();
        assert!(
            missing.is_empty() && stale.is_empty(),
            "tests/fixtures/wire/current/{direction}/ must hold one <type>.json per {definition} \
             variant; missing {missing:?}, no longer a variant {stale:?}"
        );
    }
}

#[test]
fn every_fixture_in_the_window_decodes() {
    for (_, dir) in releases_in_window() {
        each_fixture(&dir, |direction, path| match direction {
            "client" => decode(path, &ClientMessage::Unknown).map(drop),
            _ => decode(path, &ServerMessage::Unknown).map(drop),
        });
    }
}

/// Encoding what was decoded gives the same message back, so a fixture is something this build
/// actually writes, not only something it tolerates.
#[test]
fn current_fixtures_round_trip_through_the_production_encoder() {
    fn round_trip<T>(path: &Path, unknown: &T) -> Result<(), String>
    where
        T: DeserializeOwned + Serialize + std::fmt::Debug + PartialEq,
    {
        let message = decode(path, unknown)?;
        let encoded = serde_json::to_value(&message).map_err(|err| err.to_string())?;
        let again: T = serde_json::from_value(encoded).map_err(|err| err.to_string())?;
        (again == message)
            .then_some(())
            .ok_or_else(|| format!("re-encoding changed the message: {again:?}"))
    }
    each_fixture(
        &wire_fixtures().join("current"),
        |direction, path| match direction {
            "client" => round_trip(path, &ClientMessage::Unknown),
            _ => round_trip(path, &ServerMessage::Unknown),
        },
    );
}

/// What this build sends to a peer that negotiated nothing optional must be valid for every
/// generation in the window: the peer may be the oldest build this one still talks to.
#[test]
fn baseline_messages_validate_against_every_generation_in_the_window() {
    for generation in MIN_SUPPORTED_PROTOCOL..=PROTOCOL_VERSION {
        let mut schema =
            read_json(&wire_fixtures().join(format!("schema/gen-{generation}.schema.json")));
        let validators: Vec<_> = DIRECTIONS
            .iter()
            .map(|(direction, definition)| {
                schema["$ref"] = format!("#/$defs/{definition}").into();
                let validator = jsonschema::validator_for(&schema)
                    .unwrap_or_else(|err| panic!("gen-{generation} schema: {err}"));
                (*direction, validator)
            })
            .collect();
        assert!(
            !validators[0]
                .1
                .is_valid(&serde_json::json!({"type": "attach", "label": "no session"})),
            "the gen-{generation} schema must reject an attach with no session, or this check \
             validates nothing"
        );
        each_fixture(&wire_fixtures().join("current"), |direction, path| {
            let encoded = match direction {
                "client" => {
                    let message = decode(path, &ClientMessage::Unknown)?;
                    if message.required_capability().is_some() {
                        return Ok(());
                    }
                    serde_json::to_value(message)
                }
                _ => {
                    let message = decode(path, &ServerMessage::Unknown)?;
                    if message.required_capability().is_some() {
                        return Ok(());
                    }
                    serde_json::to_value(message)
                }
            }
            .map_err(|err| err.to_string())?;
            let (_, validator) = validators.iter().find(|(d, _)| *d == direction).unwrap();
            let errors: Vec<String> = validator
                .iter_errors(&encoded)
                .map(|error| format!("{} at {}", error, error.instance_path()))
                .collect();
            errors.is_empty().then_some(()).ok_or_else(|| {
                format!("invalid for generation {generation}: {}", errors.join("; "))
            })
        });
    }
}
