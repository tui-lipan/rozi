//! The session wire protocol's JSON Schema, generated from the types that serialize it.
//!
//! Compiled only under the `schema-gen` feature, like [`crate::api_schema`]. The schema is a
//! checked-in developer artifact rather than a published contract: it is how a reviewer sees the
//! wire move, and the baseline the compatibility classifier compares every change against. See
//! `dev/wire-compatibility.md`.
//!
//! Binary frames (pane bytes, acknowledgements, presentation interest) are not JSON and are not
//! described here; their kinds live in [`super::constants`]. Two framework types that ride inside
//! control frames, [`tui_lipan::Color`] and [`tui_lipan::ProgramStatusSnapshot`], are opaque to the
//! schema; the golden wire corpus pins their JSON shape instead.

use schemars::{JsonSchema, SchemaGenerator};
use serde_json::{Map, Value, json};

/// Where the generated document lives, relative to the repository root.
pub const SCHEMA_PATH: &str = "schemas/session-wire.schema.json";

/// Build the schema bundle: one document whose `$defs` holds every session wire type.
pub fn bundle() -> Value {
    let mut generator = SchemaGenerator::default();
    let mut roots = Map::new();

    // Both directions of a session connection, and the capability set the handshake carries.
    add::<super::ClientMessage>(&mut generator, &mut roots);
    add::<super::ServerMessage>(&mut generator, &mut roots);
    add::<super::Capabilities>(&mut generator, &mut roots);
    // The documents carried inside them, which have versions of their own.
    add::<crate::layout::shared::SharedLayout>(&mut generator, &mut roots);

    let mut defs = generator.take_definitions(false);
    for (name, schema) in roots {
        defs.entry(name).or_insert(schema);
    }

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "Rozi session wire protocol",
        "description": concat!(
            "Control frames exchanged between a rozi session server and its clients, CLI callers, ",
            "and remote proxies. Generated; see dev/wire-compatibility.md for the rules every ",
            "change to it follows.",
        ),
        "x-rozi-wire-generation": super::PROTOCOL_VERSION,
        "x-rozi-wire-generation-min": super::MIN_SUPPORTED_PROTOCOL,
        "x-rozi-capabilities": super::Capability::ALL
            .iter()
            .map(|cap| cap.as_str())
            .collect::<Vec<_>>(),
        "x-rozi-shared-layout-version": crate::layout::shared::SHARED_LAYOUT_VERSION,
        "$defs": defs,
    })
}

/// Register `T` and record its own schema under its type name. See [`crate::api_schema`].
fn add<T: JsonSchema>(generator: &mut SchemaGenerator, roots: &mut Map<String, Value>) {
    let schema = generator.subschema_for::<T>();
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

    /// The checked-in schema is what a reviewer reads to see the wire move, and what the
    /// compatibility classifier compares against. Failing here is the reminder to regenerate it in
    /// the same change.
    #[test]
    fn the_checked_in_wire_schema_still_describes_the_types() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SCHEMA_PATH);
        let checked_in = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            checked_in,
            document(),
            "{} is out of date - regenerate it with \
             `cargo run --features schema-gen --bin rozi-api-schema`",
            path.display()
        );
    }

    /// Both message enums must be internally tagged and keep their catch-all, or an unknown message
    /// from a newer peer would close the connection again.
    #[test]
    fn both_message_enums_accept_an_unknown_type() {
        let bundle = bundle();
        for name in ["ClientMessage", "ServerMessage"] {
            let text = bundle["$defs"][name].to_string();
            assert!(
                text.contains("\"unknown\""),
                "{name} must keep its `Unknown` catch-all variant"
            );
        }
    }
}
