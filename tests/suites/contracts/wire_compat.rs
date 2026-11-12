//! The session wire's evolution rules, checked against the frozen schema of each generation.
//!
//! `schemas/session-wire.schema.json` is regenerated from the wire types on every change (CI fails
//! when it is stale). `tests/fixtures/wire/schema/gen-<N>.schema.json` is the shape generation `N`
//! shipped with, frozen when the generation was introduced. Every change since then must be one the
//! evolution rules in `dev/wire-compatibility.md` allow without a generation bump: added optional
//! fields, added message variants that require a capability, and added vocabulary values where the
//! vocabulary has an `unknown` fallback. Anything else - a removed or renamed field or variant, a
//! type change, a new required field, a closed vocabulary that grew - is flagged here and needs a
//! new generation.
//!
//! A new message variant is only allowed when it answers `required_capability` with a capability.
//! The schema cannot say that, so the check decodes the variant's golden fixture from
//! `tests/fixtures/wire/current/` and asks the production type.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use rozi::session::protocol::{
    Capability, ClientMessage, GATED_VOCABULARY, MIN_SUPPORTED_PROTOCOL, PROTOCOL_VERSION,
    ServerMessage,
};
use serde_json::Value;

/// The message enums, whose new variants are checked against `required_capability`.
const MESSAGE_ROOTS: [&str; 2] = ["ClientMessage", "ServerMessage"];
/// Every root the wire carries. A type reached from none of these is not on the wire.
const ROOTS: [&str; 3] = ["ClientMessage", "ServerMessage", "SharedLayout"];
/// Properties that usually tag a variant of an internally tagged enum. Any other property with a
/// constant value tags it too, so a new tag name is still recognised.
const TAGS: [&str; 4] = ["type", "kind", "step", "cmd"];
/// The catch-all variant a tolerant vocabulary decodes unknown values into.
const UNKNOWN: &str = "unknown";
/// Vocabularies that stay closed in the schema, because the control API publishes them, but that
/// the session wire decodes tolerantly through `session::protocol::tolerant` (or Serde's
/// `#[serde(other)]` on `ControlCommand`). Each entry's variants stay compatible across peers.
const TOLERANT: [&str; 5] = [
    "AgentAuthority",
    "AgentState",
    "ControlRequest",
    "DetectedAgentState",
    "PublisherKind",
];

fn repo() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn read_json(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|err| panic!("parse {}: {err}", path.display()))
}

fn baseline_path(generation: u32) -> PathBuf {
    repo().join(format!(
        "tests/fixtures/wire/schema/gen-{generation}.schema.json"
    ))
}

/// What the classifier found between a generation's frozen schema and the current one.
#[derive(Debug, Default)]
struct Classification {
    /// Changes the rules only allow with a generation bump, each with the path it happened at.
    breaking: Vec<String>,
    /// Message variants added since the baseline, by root and tag. Each must require a capability.
    new_variants: BTreeMap<&'static str, BTreeSet<String>>,
}

fn classify(baseline: &Value, current: &Value) -> Classification {
    let gated: Vec<(&str, &str)> = GATED_VOCABULARY
        .iter()
        .map(|gated| (gated.vocabulary, gated.value))
        .collect();
    classify_with(baseline, current, &gated)
}

/// [`classify`], with the vocabulary values that are gated on a capability given as
/// `(definition, value)` pairs.
fn classify_with(baseline: &Value, current: &Value, gated: &[(&str, &str)]) -> Classification {
    let mut walk = Walk {
        baseline,
        current,
        gated,
        seen: BTreeSet::new(),
        out: Classification::default(),
    };
    for root in ROOTS {
        let reference = format!("#/$defs/{root}");
        let base = serde_json::json!({ "$ref": reference });
        let cur = base.clone();
        match current.pointer(&format!("/$defs/{root}")) {
            Some(_) => walk.compare(
                &base,
                &cur,
                root,
                MESSAGE_ROOTS.iter().find(|r| **r == root),
            ),
            None => walk.out.breaking.push(format!("{root}: removed")),
        }
    }
    let names = |doc: &Value| -> BTreeSet<String> {
        doc["x-rozi-capabilities"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|name| name.as_str().map(str::to_owned))
            .collect()
    };
    for removed in names(baseline).difference(&names(current)) {
        walk.out
            .breaking
            .push(format!("capability `{removed}`: removed"));
    }
    walk.out
}

struct Walk<'a> {
    baseline: &'a Value,
    current: &'a Value,
    /// Vocabulary values added within this generation behind a capability.
    gated: &'a [(&'a str, &'a str)],
    /// Pairs of definitions already compared, so recursive types terminate.
    seen: BTreeSet<(String, String)>,
    out: Classification,
}

/// A schema node with its references resolved and nullability split off.
struct Node<'a> {
    schema: &'a Value,
    nullable: bool,
}

fn resolve<'a>(doc: &'a Value, mut schema: &'a Value) -> &'a Value {
    while let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        let pointer = reference.trim_start_matches('#');
        schema = doc
            .pointer(pointer)
            .unwrap_or_else(|| panic!("dangling schema reference {reference}"));
    }
    schema
}

fn node<'a>(doc: &'a Value, schema: &'a Value) -> Node<'a> {
    let schema = resolve(doc, schema);
    // `Option<T>` is `anyOf: [T, {type: null}]` for a referenced T.
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array)
        && options.len() == 2
        && let Some(null) = options
            .iter()
            .position(|o| o.get("type") == Some(&"null".into()))
    {
        let inner = node(doc, &options[1 - null]);
        return Node {
            schema: inner.schema,
            nullable: true,
        };
    }
    let nullable = schema
        .get("type")
        .and_then(Value::as_array)
        .is_some_and(|types| types.iter().any(|t| t == "null"));
    Node { schema, nullable }
}

/// The JSON types a node admits, without `null`.
fn types(schema: &Value) -> BTreeSet<String> {
    match schema.get("type") {
        Some(Value::String(one)) => [one.clone()].into(),
        Some(Value::Array(many)) => many
            .iter()
            .filter_map(Value::as_str)
            .filter(|t| *t != "null")
            .map(str::to_owned)
            .collect(),
        _ => BTreeSet::new(),
    }
}

/// The variants of a union, keyed by the value that tells them apart: the tag of an internally
/// tagged variant, the constant of a unit variant, or the position otherwise. A plain `enum` of
/// strings is the same vocabulary written another way (schemars picks one by whether the variants
/// carry docs), so it is read as a union of constants.
fn variants(schema: &Value) -> Option<BTreeMap<String, Value>> {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        return Some(values.iter().map(constant_variant).collect());
    }
    let list = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))?
        .as_array()?;
    let mut out = BTreeMap::new();
    for (index, variant) in list.iter().enumerate() {
        // A documented catch-all beside undocumented values comes out as `oneOf: [{enum}, {const}]`.
        if let Some(values) = variant.get("enum").and_then(Value::as_array) {
            out.extend(values.iter().map(constant_variant));
        } else {
            let key = variant_key(variant).unwrap_or(format!("#{index}"));
            out.insert(key, variant.clone());
        }
    }
    Some(out)
}

/// One value of a string vocabulary as a variant of its own.
fn constant_variant(value: &Value) -> (String, Value) {
    let key = value
        .as_str()
        .map_or_else(|| value.to_string(), str::to_owned);
    let kind = if value.is_string() {
        "string"
    } else {
        "integer"
    };
    (key, serde_json::json!({ "const": value, "type": kind }))
}

fn variant_key(variant: &Value) -> Option<String> {
    if let Some(constant) = variant.get("const").and_then(Value::as_str) {
        return Some(constant.to_owned());
    }
    if let Some(values) = variant.get("enum").and_then(Value::as_array)
        && let [Value::String(only)] = values.as_slice()
    {
        return Some(only.clone());
    }
    let properties = variant.get("properties")?.as_object()?;
    let constant = |name: &str| {
        properties
            .get(name)
            .and_then(|p| p.get("const"))
            .and_then(Value::as_str)
            .map(|value| format!("{name}={value}"))
    };
    TAGS.iter()
        .find_map(|tag| constant(tag))
        .or_else(|| properties.keys().find_map(|name| constant(name)))
}

fn tag_value(key: &str) -> &str {
    key.split_once('=').map_or(key, |(_, value)| value)
}

fn is_verified_client_only_command(cmd: &str) -> bool {
    rozi::control::ControlCommand::CLIENT_ONLY_WIRE_NAMES
        .iter()
        .any(|&(name, build)| {
            if name != cmd {
                return false;
            }
            let built = build();
            let serialized_cmd = serde_json::to_value(&built)
                .ok()
                .and_then(|v| v.get("cmd").and_then(Value::as_str).map(str::to_owned));
            serialized_cmd.as_deref() == Some(cmd)
                && built.required_session_capability().is_none()
                && rozi::session::server::session_control_unsupported(&built).is_some()
                && rozi::session::server::attached_control_refusal(&built, false).is_some()
                && rozi::session::server::attached_control_refusal(&built, true).is_some()
        })
}

fn definition_name(schema: &Value) -> Option<String> {
    schema
        .get("$ref")
        .and_then(Value::as_str)
        .map(|r| r.rsplit('/').next().unwrap_or(r).to_owned())
}

impl Walk<'_> {
    fn flag(&mut self, path: &str, what: impl std::fmt::Display) {
        self.out.breaking.push(format!("{path}: {what}"));
    }

    fn compare(&mut self, base: &Value, cur: &Value, path: &str, root: Option<&&'static str>) {
        let vocabulary = definition_name(cur);
        let decoded_tolerantly = vocabulary
            .as_deref()
            .is_some_and(|name| TOLERANT.contains(&name));
        if let (Some(b), Some(c)) = (definition_name(base), definition_name(cur))
            && !self.seen.insert((b, c))
        {
            return;
        }
        let b = node(self.baseline, base);
        let c = node(self.current, cur);
        if b.nullable && !c.nullable {
            self.flag(path, "no longer accepts null");
        }
        if !b.nullable && c.nullable {
            self.flag(path, "may now be null, which an older peer cannot decode");
        }
        let (b, c) = (b.schema, c.schema);

        // An opaque schema (`{}` or `true`) admits anything on either side.
        if b.as_object()
            .is_some_and(|o| o.keys().all(|k| k == "description"))
            || b == &Value::Bool(true)
        {
            return;
        }

        if let Some(base_variants) = variants(b) {
            let Some(cur_variants) = variants(c) else {
                self.flag(path, "is no longer a union");
                return;
            };
            let tolerant =
                decoded_tolerantly || cur_variants.keys().any(|key| tag_value(key) == UNKNOWN);
            for (key, base_variant) in &base_variants {
                match cur_variants.get(key) {
                    Some(cur_variant) => {
                        let path = format!("{path}.{}", tag_value(key));
                        self.compare(base_variant, cur_variant, &path, None);
                    }
                    None => self.flag(path, format!("variant `{}` removed", tag_value(key))),
                }
            }
            for key in cur_variants
                .keys()
                .filter(|k| !base_variants.contains_key(*k))
            {
                match root {
                    Some(root) => {
                        self.out
                            .new_variants
                            .entry(*root)
                            .or_default()
                            .insert(tag_value(key).to_owned());
                    }
                    // The catch-all itself is what lets a vocabulary grow.
                    None if tag_value(key) == UNKNOWN => {}
                    None if tolerant => {
                        let value = tag_value(key);
                        let gated = self.gated.iter().any(|(gated_in, gated_value)| {
                            Some(*gated_in) == vocabulary.as_deref() && *gated_value == value
                        });
                        let client_only_command = vocabulary.as_deref() == Some("ControlRequest")
                            && is_verified_client_only_command(value);
                        if !gated && !client_only_command {
                            self.flag(
                                path,
                                format!(
                                    "variant `{value}` added without a capability; list it in \
                                     GATED_VOCABULARY with the capability its sender checks"
                                ),
                            );
                        }
                    }
                    None => self.flag(
                        path,
                        format!(
                            "variant `{}` added to a vocabulary without an `unknown` fallback",
                            tag_value(key)
                        ),
                    ),
                }
            }
            return;
        }

        if b.get("const") != c.get("const") {
            self.flag(
                path,
                format!("constant changed from {} to {}", b["const"], c["const"]),
            );
        }
        let (base_types, cur_types) = (types(b), types(c));
        if base_types != cur_types {
            self.flag(
                path,
                format!("type changed from {base_types:?} to {cur_types:?}"),
            );
            return;
        }
        if b.get("format") != c.get("format") {
            self.flag(
                path,
                format!("format changed from {} to {}", b["format"], c["format"]),
            );
        }

        if let Some(base_props) = b.get("properties").and_then(Value::as_object) {
            let empty = serde_json::Map::new();
            let cur_props = c
                .get("properties")
                .and_then(Value::as_object)
                .unwrap_or(&empty);
            for (name, base_prop) in base_props {
                match cur_props.get(name) {
                    Some(cur_prop) => {
                        self.compare(base_prop, cur_prop, &format!("{path}.{name}"), None)
                    }
                    None => self.flag(path, format!("field `{name}` removed")),
                }
            }
            let required = |schema: &Value| -> BTreeSet<String> {
                schema["required"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            };
            let (base_required, cur_required) = (required(b), required(c));
            for name in cur_required.difference(&base_required) {
                if base_props.contains_key(name) {
                    self.flag(path, format!("field `{name}` became required"));
                } else {
                    self.flag(path, format!("new required field `{name}`"));
                }
            }
        }

        for key in ["items", "additionalProperties"] {
            if let (Some(base_inner), Some(cur_inner)) = (b.get(key), c.get(key))
                && base_inner.is_object()
            {
                self.compare(base_inner, cur_inner, &format!("{path}[]"), None);
            }
        }
        if let (Some(base_tuple), Some(cur_tuple)) = (
            b.get("prefixItems").and_then(Value::as_array),
            c.get("prefixItems").and_then(Value::as_array),
        ) {
            if base_tuple.len() != cur_tuple.len() {
                self.flag(path, "tuple length changed");
            }
            for (index, (base_item, cur_item)) in base_tuple.iter().zip(cur_tuple).enumerate() {
                self.compare(base_item, cur_item, &format!("{path}[{index}]"), None);
            }
        }
    }
}

/// Decode a new variant's golden fixture and ask the production type what it requires.
fn required_capability_of(root: &str, tag: &str) -> Result<Option<Capability>, String> {
    let dir = match root {
        "ClientMessage" => "client",
        _ => "server",
    };
    let path = repo().join(format!("tests/fixtures/wire/current/{dir}/{tag}.json"));
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "{root}::{tag} is new since the frozen schema but has no golden fixture at {}",
            path.display()
        )
    })?;
    let capability = match root {
        "ClientMessage" => serde_json::from_str::<ClientMessage>(&text)
            .map_err(|err| format!("{}: {err}", path.display()))?
            .required_capability(),
        _ => serde_json::from_str::<ServerMessage>(&text)
            .map_err(|err| format!("{}: {err}", path.display()))?
            .required_capability(),
    };
    Ok(capability)
}

#[test]
fn every_generation_in_the_window_has_a_frozen_schema() {
    for generation in MIN_SUPPORTED_PROTOCOL..=PROTOCOL_VERSION {
        let path = baseline_path(generation);
        assert!(
            path.exists(),
            "generation {generation} is in the support window but {} is missing. Freeze it from \
             schemas/session-wire.schema.json when the generation is introduced; see \
             dev/wire-compatibility.md",
            path.display()
        );
        assert_eq!(
            read_json(&path)["x-rozi-wire-generation"],
            generation,
            "{} must be the schema of generation {generation}",
            path.display()
        );
    }
}

/// The current wire may only have moved in ways generation `PROTOCOL_VERSION` already allows.
#[test]
fn the_wire_only_changed_in_ways_this_generation_allows() {
    let baseline = read_json(&baseline_path(PROTOCOL_VERSION));
    let current = read_json(&repo().join("schemas/session-wire.schema.json"));
    let found = classify(&baseline, &current);

    let mut problems = found.breaking;
    for (root, tags) in &found.new_variants {
        for tag in tags {
            match required_capability_of(root, tag) {
                Ok(Some(_)) => {}
                Ok(None) => problems.push(format!(
                    "{root}::{tag} is new since generation {PROTOCOL_VERSION} but \
                     `required_capability` answers None; gate it on a Capability"
                )),
                Err(problem) => problems.push(problem),
            }
        }
    }
    assert!(
        problems.is_empty(),
        "these wire changes need a new generation (or a capability) under \
         dev/wire-compatibility.md:\n  {}",
        problems.join("\n  ")
    );
}

#[cfg(test)]
mod classifier {
    use super::*;
    use serde_json::json;

    /// Drop `value` from a string vocabulary, whichever way the schema lists it.
    fn remove_value(schema: &mut Value, value: &str) {
        match schema {
            Value::Array(items) => {
                items.retain(|item| item != value && item.get("const").is_none_or(|c| c != value));
                items.iter_mut().for_each(|item| remove_value(item, value));
            }
            Value::Object(fields) => fields.values_mut().for_each(|v| remove_value(v, value)),
            _ => {}
        }
    }

    fn doc(defs: Value) -> Value {
        json!({ "$defs": defs, "x-rozi-capabilities": ["runtime-metrics"] })
    }

    fn message(variants: Vec<Value>) -> Value {
        json!({ "oneOf": variants })
    }

    fn variant(tag: &str, properties: Value, required: &[&str]) -> Value {
        let mut properties = properties;
        properties["type"] = json!({ "type": "string", "const": tag });
        let mut required: Vec<&str> = required.to_vec();
        required.push("type");
        json!({ "type": "object", "properties": properties, "required": required })
    }

    fn wire(client: Vec<Value>, extra: Value) -> Value {
        let mut defs = json!({
            "ClientMessage": message(client),
            "ServerMessage": message(vec![variant("ping", json!({"seq": {"type": "integer", "format": "uint64"}}), &["seq"])]),
            "SharedLayout": { "type": "object", "properties": {}, "required": [] },
        });
        for (name, schema) in extra.as_object().unwrap() {
            defs[name] = schema.clone();
        }
        doc(defs)
    }

    fn rename() -> Value {
        variant("rename", json!({"name": {"type": "string"}}), &["name"])
    }

    #[test]
    fn identical_schemas_classify_clean() {
        let schema = wire(vec![rename()], json!({}));
        let found = classify(&schema, &schema);
        assert!(found.breaking.is_empty(), "{:?}", found.breaking);
        assert!(found.new_variants.is_empty());
    }

    #[test]
    fn an_added_optional_field_is_allowed_and_a_required_one_is_not() {
        let base = wire(vec![rename()], json!({}));
        let optional = wire(
            vec![variant(
                "rename",
                json!({"name": {"type": "string"}, "why": {"type": ["string", "null"]}}),
                &["name"],
            )],
            json!({}),
        );
        assert!(classify(&base, &optional).breaking.is_empty());
        let required = wire(
            vec![variant(
                "rename",
                json!({"name": {"type": "string"}, "why": {"type": "string"}}),
                &["name", "why"],
            )],
            json!({}),
        );
        assert_eq!(
            classify(&base, &required).breaking,
            ["ClientMessage.rename: new required field `why`"]
        );
    }

    #[test]
    fn removals_renames_and_type_changes_are_breaking() {
        let base = wire(vec![rename()], json!({}));
        let removed = wire(vec![variant("rename", json!({}), &[])], json!({}));
        assert_eq!(
            classify(&base, &removed).breaking,
            ["ClientMessage.rename: field `name` removed"]
        );
        let renamed = wire(
            vec![variant(
                "retitle",
                json!({"name": {"type": "string"}}),
                &["name"],
            )],
            json!({}),
        );
        let found = classify(&base, &renamed);
        assert_eq!(found.breaking, ["ClientMessage: variant `rename` removed"]);
        assert_eq!(
            found.new_variants["ClientMessage"],
            ["retitle".to_string()].into()
        );
        let retyped = wire(
            vec![variant(
                "rename",
                json!({"name": {"type": "integer"}}),
                &["name"],
            )],
            json!({}),
        );
        assert_eq!(
            classify(&base, &retyped).breaking,
            [r#"ClientMessage.rename.name: type changed from {"string"} to {"integer"}"#]
        );
        let nullable = wire(
            vec![variant(
                "rename",
                json!({"name": {"type": ["string", "null"]}}),
                &["name"],
            )],
            json!({}),
        );
        assert_eq!(
            classify(&base, &nullable).breaking,
            ["ClientMessage.rename.name: may now be null, which an older peer cannot decode"]
        );
    }

    #[test]
    fn a_new_message_variant_is_reported_for_the_capability_check() {
        let base = wire(vec![rename()], json!({}));
        let grown = wire(
            vec![rename(), variant("teleport", json!({}), &[])],
            json!({}),
        );
        let found = classify(&base, &grown);
        assert!(found.breaking.is_empty(), "{:?}", found.breaking);
        assert_eq!(
            found.new_variants["ClientMessage"],
            ["teleport".to_string()].into()
        );
    }

    #[test]
    fn a_vocabulary_may_grow_only_behind_an_unknown_fallback() {
        let state = |values: &[&str]| json!({ "type": "string", "enum": values });
        let with = |values: &[&str]| {
            wire(
                vec![variant(
                    "report",
                    json!({"state": {"$ref": "#/$defs/State"}}),
                    &["state"],
                )],
                json!({ "State": state(values) }),
            )
        };
        let base = with(&["idle", "working"]);
        assert_eq!(
            classify(&base, &with(&["idle", "working", "sleeping"])).breaking,
            [
                "ClientMessage.report.state: variant `sleeping` added to a vocabulary without an `unknown` fallback"
            ]
        );
        let tolerant_base = with(&["idle", "working", "unknown"]);
        let grown = with(&["idle", "working", "unknown", "sleeping"]);
        // Behind the fallback, a new value still needs the capability its sender checks.
        assert_eq!(
            classify_with(&tolerant_base, &grown, &[]).breaking.len(),
            1,
            "an unregistered value is flagged"
        );
        assert!(
            classify_with(&tolerant_base, &grown, &[("State", "sleeping")])
                .breaking
                .is_empty()
        );
        // Gaining the catch-all is how a closed vocabulary becomes tolerant.
        assert!(classify(&base, &tolerant_base).breaking.is_empty());
        assert_eq!(
            classify(&base, &with(&["idle"])).breaking,
            ["ClientMessage.report.state: variant `working` removed"]
        );
    }

    /// The walker has to reach deep into the real generated schema, through references, optional
    /// wrappers, and nested enums, or the checks above prove nothing about the actual wire.
    #[test]
    fn the_real_schema_is_walked_through_references() {
        let base = read_json(&baseline_path(PROTOCOL_VERSION));
        let mut current = base.clone();
        let attach = current["$defs"]["ClientMessage"]["oneOf"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|v| v["properties"]["type"]["const"] == "attach")
            .unwrap();
        attach["properties"]
            .as_object_mut()
            .unwrap()
            .remove("label");
        remove_value(&mut current["$defs"]["WireChangeState"], "modified");
        let found = classify(&base, &current);
        assert!(
            found
                .breaking
                .contains(&"ClientMessage.attach: field `label` removed".to_string()),
            "{:?}",
            found.breaking
        );
        assert!(
            found
                .breaking
                .iter()
                .any(|problem| problem.contains("git_staged")
                    && problem.contains("`modified` removed")),
            "a vocabulary nested behind an Option and a reference is checked: {:?}",
            found.breaking
        );
    }

    #[test]
    fn a_removed_capability_name_is_breaking() {
        let base = wire(vec![rename()], json!({}));
        let mut current = base.clone();
        current["x-rozi-capabilities"] = json!([]);
        assert_eq!(
            classify(&base, &current).breaking,
            ["capability `runtime-metrics`: removed"]
        );
    }
}
