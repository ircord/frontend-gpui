//! Every event type in the pinned contract decodes to its own variant.
//!
//! `frames.jsonl` holds one frame per event type. Each frame is checked
//! against `openapi.json` (every required field present, every enum value
//! known, all the way down), so the fixture cannot drift from the contract;
//! then it must decode to something other than `Unknown` or `Durable::Other`.
//! A backend bump that adds an event type fails here until it is decoded.

use std::collections::BTreeSet;

use ircord_contract::event::{Durable, SERVER_EVENT_TYPES, ServerEvent};
use serde_json::Value;

const FRAMES: &str = include_str!("frames.jsonl");
const SPEC: &str = include_str!("../openapi.json");

fn spec() -> Value {
    serde_json::from_str(SPEC).unwrap()
}

/// `type` → schema name, from the contract's `ServerEvent` discriminator.
fn contract_types(spec: &Value) -> Vec<(String, String)> {
    spec["components"]["schemas"]["ServerEvent"]["discriminator"]["mapping"]
        .as_object()
        .expect("ServerEvent has a discriminator mapping")
        .iter()
        .map(|(kind, r)| {
            let name = r.as_str().unwrap().rsplit('/').next().unwrap();
            (kind.clone(), name.to_string())
        })
        .collect()
}

fn resolve<'a>(spec: &'a Value, schema: &'a Value) -> &'a Value {
    match schema.get("$ref").and_then(Value::as_str) {
        Some(r) => {
            let name = r.rsplit('/').next().unwrap();
            resolve(spec, &spec["components"]["schemas"][name])
        }
        None => schema,
    }
}

/// Check `value` against `schema`: required fields, enum values, nested objects and arrays.
fn conforms(spec: &Value, schema: &Value, value: &Value, at: &str, errors: &mut Vec<String>) {
    let schema = resolve(spec, schema);
    if value.is_null() {
        return;
    }
    for key in ["anyOf", "oneOf"] {
        if let Some(options) = schema.get(key).and_then(Value::as_array) {
            let fits = options.iter().any(|o| {
                let mut e = Vec::new();
                let o = resolve(spec, o);
                if o.get("type").and_then(Value::as_str) == Some("null") {
                    return false;
                }
                conforms(spec, o, value, at, &mut e);
                e.is_empty()
            });
            if !fits {
                errors.push(format!("{at}: fits none of {key}"));
            }
            return;
        }
    }
    let types: Vec<&str> = match schema.get("type") {
        Some(Value::String(t)) => vec![t.as_str()],
        Some(Value::Array(ts)) => ts.iter().filter_map(Value::as_str).collect(),
        _ => Vec::new(),
    };
    let fits_type = |t: &str| match t {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "array" => value.is_array(),
        "object" => value.is_object(),
        _ => true,
    };
    if !types.is_empty() && !types.iter().any(|t| fits_type(t)) {
        errors.push(format!("{at}: {value} is not {types:?}"));
        return;
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.contains(value)
    {
        errors.push(format!("{at}: {value} is not one of {values:?}"));
    }
    if let Some(props) = schema.get("properties").and_then(Value::as_object) {
        let Some(object) = value.as_object() else {
            errors.push(format!("{at}: expected an object"));
            return;
        };
        for required in schema["required"].as_array().into_iter().flatten() {
            let key = required.as_str().unwrap();
            if !object.contains_key(key) {
                errors.push(format!("{at}: missing required `{key}`"));
            }
        }
        for (key, v) in object {
            match props.get(key) {
                Some(p) => conforms(spec, p, v, &format!("{at}.{key}"), errors),
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    errors.push(format!("{at}: `{key}` is not in the contract"))
                }
                None => {}
            }
        }
    }
    if schema.get("properties").is_none()
        && let (Some(each @ Value::Object(_)), Some(object)) =
            (schema.get("additionalProperties"), value.as_object())
    {
        for (key, v) in object {
            conforms(spec, each, v, &format!("{at}.{key}"), errors);
        }
    }
    if let (Some(items), Some(array)) = (schema.get("items"), value.as_array()) {
        for (i, v) in array.iter().enumerate() {
            conforms(spec, items, v, &format!("{at}[{i}]"), errors);
        }
    }
}

fn frames() -> Vec<Value> {
    FRAMES
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

#[test]
fn the_fixture_has_one_conforming_frame_per_contract_type() {
    let spec = spec();
    let types = contract_types(&spec);
    let frames = frames();
    let mut errors = Vec::new();
    for (kind, schema) in &types {
        let Some(frame) = frames.iter().find(|f| f["type"] == kind.as_str()) else {
            errors.push(format!("frames.jsonl has no `{kind}` frame"));
            continue;
        };
        conforms(
            &spec,
            &spec["components"]["schemas"][schema],
            frame,
            kind,
            &mut errors,
        );
    }
    let known: BTreeSet<&str> = types.iter().map(|(k, _)| k.as_str()).collect();
    for frame in &frames {
        let kind = frame["type"].as_str().unwrap();
        if !known.contains(kind) {
            errors.push(format!(
                "frames.jsonl has `{kind}`, which the contract lacks"
            ));
        }
    }
    assert!(errors.is_empty(), "{}", errors.join("\n"));
}

#[test]
fn the_envelope_lists_exactly_the_contract_types() {
    let spec = spec();
    let contract: BTreeSet<String> = contract_types(&spec).into_iter().map(|(k, _)| k).collect();
    let decoded: BTreeSet<String> = SERVER_EVENT_TYPES.iter().map(|s| s.to_string()).collect();
    assert_eq!(contract, decoded);
}

#[test]
fn every_frame_decodes_to_its_own_variant() {
    for frame in frames() {
        let text = frame.to_string();
        let kind = frame["type"].as_str().unwrap();
        let event = ServerEvent::decode(&text).unwrap();
        match &event {
            ServerEvent::Unknown { .. }
            | ServerEvent::Durable {
                event: Durable::Other(_),
                ..
            } => panic!("`{kind}` did not decode: {event:?}"),
            _ => {}
        }
        assert_eq!(event.kind(), kind);
        assert_eq!(
            event.seq(),
            frame["seq"].as_i64(),
            "`{kind}` keeps its sequence number"
        );
    }
}
