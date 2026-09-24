//! The control API's JSON Schema, generated from the types that serialize the wire format.
//!
//! Compiled only under the `schema-gen` feature. Rozi never computes this at runtime: the schema
//! ships as a checked-in file, and `cargo run --features schema-gen --bin rozi-api-schema` is what
//! rewrites it. Generating from the real `Serialize` types rather than describing them a second
//! time by hand is the point - a renamed field, a new enum variant, or a changed
//! `skip_serializing_if` all show up as a diff in that file, in the same pull request that caused
//! them.

use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

/// Where the generated document lives, relative to the repository root.
pub const SCHEMA_PATH: &str = "docs/schema/rozi-control-v1.schema.json";

/// Build the schema bundle: one document whose `$defs` holds every public control API type.
///
/// A bundle rather than a schema per type, because the types reference each other and a consumer
/// validating a response wants the agent record and the pane record already resolved beside it.
pub fn bundle() -> Value {
    let mut generator = SchemaGenerator::default();
    let mut roots = Map::new();

    // Everything a caller sends, and the envelope everything comes back in.
    add::<crate::control::ControlRequest>(&mut generator, &mut roots);
    add::<crate::control::ControlCommand>(&mut generator, &mut roots);
    add::<crate::control::ControlResponse>(&mut generator, &mut roots);
    add::<crate::control::ControlErrorCode>(&mut generator, &mut roots);
    add::<crate::control::ApiDescription>(&mut generator, &mut roots);

    // The `data` payloads of the public commands. `ControlResponse::data` is untyped on the wire -
    // one envelope carries all of them - so the schema names each payload here instead, and a
    // consumer picks the one for the command it sent.
    // Both the element and the whole `data` value: `list-panes` and `agents list` return arrays,
    // and a consumer needs a definition naming the thing it actually received.
    add::<crate::control::PaneListPayload>(&mut generator, &mut roots);
    add::<crate::control::PaneInfo>(&mut generator, &mut roots);
    add::<crate::control::LayoutReport>(&mut generator, &mut roots);
    add::<crate::control::LayoutChange>(&mut generator, &mut roots);
    add::<crate::control::PaneClosed>(&mut generator, &mut roots);
    add::<crate::control::AgentListPayload>(&mut generator, &mut roots);
    add::<crate::control::PaneCapture>(&mut generator, &mut roots);
    add::<crate::control::UiCapture>(&mut generator, &mut roots);
    add::<crate::control::SpanFrame>(&mut generator, &mut roots);
    add::<crate::control::NewPaneAccepted>(&mut generator, &mut roots);
    add::<crate::control::PaneLoggingState>(&mut generator, &mut roots);
    add::<crate::control::AgentInfo>(&mut generator, &mut roots);
    add::<crate::control::AgentPromptAccepted>(&mut generator, &mut roots);
    add::<crate::control::AgentWaitResult>(&mut generator, &mut roots);
    add::<crate::control::SessionMetricsReport>(&mut generator, &mut roots);
    add::<crate::runtime_metrics::RuntimeMetrics>(&mut generator, &mut roots);

    // Published activity, and the events `subscribe` streams.
    add::<crate::session::protocol::PublishedRow>(&mut generator, &mut roots);
    add::<crate::events::WireEvent>(&mut generator, &mut roots);
    add::<crate::events::EventKind>(&mut generator, &mut roots);

    let mut defs = generator.take_definitions(false);
    for (name, schema) in roots {
        defs.entry(name).or_insert(schema);
    }

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$id": "https://rozi.tui-lipan.dev/schema/rozi-control-v1.schema.json",
        "title": "Rozi control API",
        "description": concat!(
            "Request, response, payload, activity, and event shapes of the `rozi` control API. ",
            "Objects do not forbid unknown properties: new fields are added over time and a ",
            "consumer is expected to ignore the ones it does not know. Enumerations are closed, ",
            "because those are the vocabularies worth validating against.",
        ),
        "x-rozi-schema-version": crate::control::API_SCHEMA_VERSION,
        "x-rozi-api-version": crate::control::CONTROL_API_VERSION,
        "$defs": defs,
    })
}

/// Register `T` and record its own schema under its type name, so the bundle names every root and
/// not just the types that happened to be referenced from another one.
fn add<T: JsonSchema>(generator: &mut SchemaGenerator, roots: &mut Map<String, Value>) {
    let schema = generator.subschema_for::<T>();
    // A type referenced by another is already a `$ref` into `$defs`; a root that nothing else
    // mentions comes back inline and has to be filed under its own name.
    if schema.as_value().get("$ref").is_none() {
        roots.insert(T::schema_name().into_owned(), schema.to_value());
    }
}

/// The schema document as it should appear on disk: pretty-printed, newline-terminated.
pub fn document() -> String {
    let mut text = serde_json::to_string_pretty(&bundle()).expect("the schema bundle serializes");
    text.push('\n');
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checked_in_path() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_PATH)
    }

    /// The published schema is the contract other software reads, and it is only worth reading if
    /// it still describes what rozi actually serializes. Renaming a field, adding an error code, or
    /// changing a `skip_serializing_if` all move this file; failing here is the reminder to commit
    /// it in the same change, where a reviewer can see the API move.
    #[test]
    fn the_checked_in_schema_still_describes_the_types() {
        let path = checked_in_path();
        let checked_in = std::fs::read_to_string(&path).unwrap_or_default();

        assert_eq!(
            checked_in,
            document(),
            "{} is out of date - regenerate it with \
             `cargo run --features schema-gen --bin rozi-api-schema`",
            path.display()
        );
    }

    /// Two rules the bundle is meant to follow, in the two directions that matter.
    ///
    /// Closed vocabularies - error codes, agent states, event names - are exactly where a schema
    /// earns its keep, so they must enumerate. Objects must not forbid unknown properties, because
    /// responses gain fields and a consumer validating against an older schema has to keep working.
    #[test]
    fn enums_are_closed_and_objects_are_not() {
        let bundle = bundle();
        let defs = bundle["$defs"].as_object().expect("the bundle has defs");

        for name in [
            "ControlErrorCode",
            "AgentState",
            "EventKind",
            "AgentAuthority",
        ] {
            assert!(
                defs[name].get("enum").is_some(),
                "{name} should enumerate its values"
            );
        }

        let mut closed = Vec::new();
        collect_closed_objects(&bundle, &mut String::new(), &mut closed);
        assert!(
            closed.is_empty(),
            "these objects forbid unknown properties, which breaks a consumer the first time a \
             response gains a field: {closed:?}"
        );
    }

    fn collect_closed_objects(value: &Value, path: &mut String, found: &mut Vec<String>) {
        match value {
            Value::Object(map) => {
                if map.get("additionalProperties") == Some(&Value::Bool(false)) {
                    found.push(path.clone());
                }
                for (key, child) in map {
                    let mark = path.len();
                    path.push('/');
                    path.push_str(key);
                    collect_closed_objects(child, path, found);
                    path.truncate(mark);
                }
            }
            Value::Array(items) => {
                for child in items {
                    collect_closed_objects(child, path, found);
                }
            }
            _ => {}
        }
    }
}
