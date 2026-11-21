//! `cargo xtask contract`: refresh `crates/contract/openapi.json` from the
//! backend checkout and regenerate `crates/contract/src/generated.rs` from it.
//!
//! The generator writes tolerant types on purpose: every struct takes
//! `#[serde(default)]` and ignores unknown fields, and every string enum has an
//! `Unknown` variant, so a newer backend never breaks an older client.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Every schema in the contract is generated, so a feature never has to touch
/// this file: it changes only when the backend is bumped. These schemas are
/// skipped because the hand-written envelope in `event.rs` replaces them: they
/// are untagged unions of every event, which serde cannot decode tolerantly.
const SKIP: &[&str] = &[
    "ServerEvent",
    "DurableEvent",
    "Event",
    "ClientEvent",
    "TraceEvent",
];

pub fn run(root: &Path, check: bool) -> Result<()> {
    let contract = root.join("crates/contract");
    let spec_path = contract.join("openapi.json");
    let commit_path = contract.join("backend-commit");
    let generated_path = contract.join("src/generated.rs");

    let backend = backend_dir(root)?;
    let backend_spec = std::fs::read(backend.join("openapi.json"))
        .with_context(|| format!("read {}/openapi.json", backend.display()))?;
    let backend_commit = git_head(&backend)?;

    if check {
        let mut stale = Vec::new();
        if std::fs::read(&spec_path).ok().as_deref() != Some(&backend_spec[..]) {
            stale.push(format!(
                "crates/contract/openapi.json differs from {}/openapi.json (backend {backend_commit})",
                backend.display()
            ));
        }
        let recorded = std::fs::read_to_string(&commit_path).unwrap_or_default();
        if recorded.trim() != backend_commit {
            stale.push(format!(
                "crates/contract/backend-commit names {} but the backend checkout is at {backend_commit}",
                recorded.trim()
            ));
        }
        let spec = std::fs::read(&spec_path).context("read committed openapi.json")?;
        let expected = generate(&serde_json::from_slice(&spec)?)?;
        let expected = rustfmt(&expected)?;
        if std::fs::read_to_string(&generated_path).ok().as_deref() != Some(expected.as_str()) {
            stale
                .push("crates/contract/src/generated.rs is not what openapi.json generates".into());
        }
        if !stale.is_empty() {
            for line in &stale {
                eprintln!("stale: {line}");
            }
            bail!("the contract is stale: run `cargo xtask contract`");
        }
        println!("contract matches backend {backend_commit}");
        return Ok(());
    }

    std::fs::write(&spec_path, &backend_spec)?;
    std::fs::write(&commit_path, format!("{backend_commit}\n"))?;
    let spec: Value = serde_json::from_slice(&backend_spec)?;
    let code = rustfmt(&generate(&spec)?)?;
    std::fs::write(&generated_path, code)?;
    println!("contract refreshed from backend {backend_commit}");
    Ok(())
}

/// The backend checkout: `IRCORD_BACKEND`, or `../ircord-backend` beside this repository.
fn backend_dir(root: &Path) -> Result<PathBuf> {
    let dir = match std::env::var_os("IRCORD_BACKEND") {
        Some(dir) => PathBuf::from(dir),
        None => root.join("../ircord-backend"),
    };
    if !dir.join("openapi.json").is_file() {
        bail!(
            "no backend checkout at {} (set IRCORD_BACKEND to the ircord-backend checkout)",
            dir.display()
        );
    }
    Ok(dir)
}

fn git_head(dir: &Path) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["rev-parse", "HEAD"])
        .output()
        .context("run git")?;
    if !out.status.success() {
        bail!("git rev-parse failed in {}", dir.display());
    }
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

fn rustfmt(code: &str) -> Result<String> {
    use std::io::Write;
    let mut child = Command::new("rustfmt")
        .args(["--edition", "2024", "--emit", "stdout"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .context("run rustfmt")?;
    child.stdin.take().unwrap().write_all(code.as_bytes())?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!("rustfmt failed on the generated contract");
    }
    Ok(String::from_utf8(out.stdout)?)
}

struct Generator<'a> {
    schemas: &'a Map<String, Value>,
    /// Named items to emit, keyed by Rust name.
    items: BTreeMap<String, String>,
    queue: Vec<String>,
    queued: BTreeSet<String>,
    /// The schema each Rust name was emitted from, so two different schemas
    /// that would get the same name fail the generator instead of one
    /// silently standing in for the other.
    sources: BTreeMap<String, Value>,
    collisions: Vec<String>,
}

pub fn generate(spec: &Value) -> Result<String> {
    let schemas = spec["components"]["schemas"]
        .as_object()
        .context("openapi.json has no components.schemas")?;
    let mut g = Generator {
        schemas,
        items: BTreeMap::new(),
        queue: Vec::new(),
        queued: BTreeSet::new(),
        sources: BTreeMap::new(),
        collisions: Vec::new(),
    };
    for name in schemas.keys() {
        if !SKIP.contains(&name.as_str()) {
            g.enqueue(name);
        }
    }
    while let Some(name) = g.queue.pop() {
        let schema = &g.schemas[&name];
        g.emit_named(&rust_name(&name), schema);
    }
    if !g.collisions.is_empty() {
        bail!(
            "two different schemas would share a Rust name: {}",
            g.collisions.join(", ")
        );
    }
    let mut out = String::new();
    out.push_str(
        "// @generated by `cargo xtask contract` from crates/contract/openapi.json. Do not edit.\n\
         #![allow(clippy::all, missing_docs)]\n\
         use serde::{Deserialize, Serialize};\n\
         use std::collections::BTreeMap;\n\n",
    );
    for item in g.items.values() {
        out.push_str(item);
        out.push('\n');
    }
    Ok(out)
}

impl Generator<'_> {
    fn enqueue(&mut self, name: &str) {
        if self.queued.insert(name.to_string()) {
            self.queue.push(name.to_string());
        }
    }

    /// Note that `name` comes from `schema`; false if it is already taken by another.
    fn claim(&mut self, name: &str, schema: &Value) -> bool {
        match self.sources.get(name) {
            Some(existing) if strip_docs(existing) == strip_docs(schema) => false,
            Some(_) => {
                self.collisions.push(name.to_string());
                false
            }
            None => {
                self.sources.insert(name.to_string(), schema.clone());
                true
            }
        }
    }

    fn emit_named(&mut self, name: &str, schema: &Value) {
        if !self.claim(name, schema) {
            return;
        }
        if let Some(values) = string_enum(schema) {
            let item = enum_item(name, &values, schema);
            self.items.insert(name.to_string(), item);
            return;
        }
        if schema.get("properties").is_some() {
            // Reserve the name before recursing so cycles terminate.
            self.items.insert(name.to_string(), String::new());
            let item = self.struct_item(name, schema);
            self.items.insert(name.to_string(), item);
            return;
        }
        let ty = self.type_of(schema, name);
        let mut item = String::new();
        doc(&mut item, schema);
        let _ = writeln!(item, "pub type {name} = {ty};");
        self.items.insert(name.to_string(), item);
    }

    fn struct_item(&mut self, name: &str, schema: &Value) -> String {
        let required: BTreeSet<&str> = schema["required"]
            .as_array()
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let mut item = String::new();
        doc(&mut item, schema);
        item.push_str(
            "#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]\n#[serde(default)]\n",
        );
        let _ = writeln!(item, "pub struct {name} {{");
        let props = schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        for (key, prop) in &props {
            if key.starts_with('$') {
                continue;
            }
            let field = field_name(key);
            let hint = format!("{name}{}", pascal(key));
            let mut ty = self.type_of(prop, &hint);
            let optional = !required.contains(key.as_str());
            if optional && !ty.starts_with("Option<") {
                ty = format!("Option<{ty}>");
            }
            doc(&mut item, prop);
            let mut attrs = Vec::new();
            if field.trim_start_matches("r#") != key {
                attrs.push(format!("rename = \"{key}\""));
            }
            if ty.starts_with("Option<") {
                attrs.push("skip_serializing_if = \"Option::is_none\"".to_string());
            }
            if !attrs.is_empty() {
                let _ = writeln!(item, "    #[serde({})]", attrs.join(", "));
            }
            let _ = writeln!(item, "    pub {field}: {ty},");
        }
        item.push_str("}\n");
        item
    }

    /// The Rust type for a schema; `hint` names any inline enum or object.
    fn type_of(&mut self, schema: &Value, hint: &str) -> String {
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            let target = r.rsplit('/').next().unwrap_or(r);
            if self.schemas.contains_key(target) {
                self.enqueue(target);
                return rust_name(target);
            }
            return "serde_json::Value".into();
        }
        for key in ["anyOf", "oneOf"] {
            if let Some(options) = schema.get(key).and_then(Value::as_array) {
                let non_null: Vec<&Value> = options
                    .iter()
                    .filter(|o| o.get("type").and_then(Value::as_str) != Some("null"))
                    .collect();
                if non_null.len() == 1 {
                    let inner = self.type_of(non_null[0], hint);
                    return if non_null.len() < options.len() {
                        option(inner)
                    } else {
                        inner
                    };
                }
                return "serde_json::Value".into();
            }
        }
        let (ty, nullable) = match schema.get("type") {
            Some(Value::String(t)) => (t.as_str(), false),
            Some(Value::Array(types)) => {
                let non_null: Vec<&str> = types
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|t| *t != "null")
                    .collect();
                let nullable = non_null.len() < types.len();
                match non_null.as_slice() {
                    [one] => (*one, nullable),
                    _ => return "serde_json::Value".into(),
                }
            }
            _ => return "serde_json::Value".into(),
        };
        let rust = match ty {
            "string" => {
                if let Some(values) = string_enum(schema) {
                    if values.len() > 1 {
                        let name = hint.to_string();
                        if self.claim(&name, schema) {
                            let item = enum_item(&name, &values, schema);
                            self.items.insert(name.clone(), item);
                        }
                        name
                    } else {
                        "String".into()
                    }
                } else {
                    "String".into()
                }
            }
            "integer" => "i64".into(),
            "number" => "f64".into(),
            "boolean" => "bool".into(),
            "array" => {
                let inner = schema
                    .get("items")
                    .map(|items| self.type_of(items, &format!("{hint}Item")))
                    .unwrap_or_else(|| "serde_json::Value".into());
                format!("Vec<{inner}>")
            }
            "object" => {
                if schema.get("properties").is_some() {
                    let name = hint.to_string();
                    self.emit_named(&name, schema);
                    name
                } else {
                    match schema.get("additionalProperties") {
                        Some(ap @ Value::Object(_)) => {
                            let inner = self.type_of(ap, &format!("{hint}Value"));
                            format!("BTreeMap<String, {inner}>")
                        }
                        _ => "BTreeMap<String, serde_json::Value>".into(),
                    }
                }
            }
            _ => "serde_json::Value".into(),
        };
        if nullable { option(rust) } else { rust }
    }
}

/// A schema without its descriptions, so the same shape documented twice is one type.
fn strip_docs(schema: &Value) -> Value {
    match schema {
        Value::Object(map) => Value::Object(
            map.iter()
                .filter(|(k, _)| !matches!(k.as_str(), "description" | "examples" | "example"))
                .map(|(k, v)| (k.clone(), strip_docs(v)))
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(strip_docs).collect()),
        other => other.clone(),
    }
}

fn option(inner: String) -> String {
    if inner.starts_with("Option<") {
        inner
    } else {
        format!("Option<{inner}>")
    }
}

fn string_enum(schema: &Value) -> Option<Vec<String>> {
    let values = schema.get("enum")?.as_array()?;
    values
        .iter()
        .map(|v| v.as_str().map(str::to_string))
        .collect::<Option<Vec<_>>>()
}

fn enum_item(name: &str, values: &[String], schema: &Value) -> String {
    let mut item = String::new();
    doc(&mut item, schema);
    item.push_str(
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]\n",
    );
    let _ = writeln!(item, "pub enum {name} {{");
    let mut used = BTreeSet::new();
    for value in values {
        let mut variant = pascal(&value.replace('+', " plus ").replace('-', " minus "));
        if variant.is_empty() || variant.starts_with(|c: char| c.is_ascii_digit()) {
            variant = format!("V{variant}");
        }
        if variant == "Unknown" {
            variant = "UnknownValue".into();
        }
        while !used.insert(variant.clone()) {
            variant.push('_');
        }
        let _ = writeln!(item, "    #[serde(rename = \"{value}\")]\n    {variant},");
    }
    item.push_str("    /// A value this client does not know yet.\n    #[default]\n    #[serde(other)]\n    Unknown,\n}\n");
    item
}

fn doc(out: &mut String, schema: &Value) {
    if let Some(text) = schema.get("description").and_then(Value::as_str) {
        for line in text.lines() {
            let _ = writeln!(out, "/// {}", line.trim());
        }
    }
}

fn rust_name(schema: &str) -> String {
    let name = pascal(schema);
    if name.is_empty() {
        "Unnamed".into()
    } else {
        name
    }
}

/// `memberIds` → `MemberIds`, `session.ready` → `SessionReady`.
fn pascal(s: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            if upper {
                out.push(c.to_ascii_uppercase());
            } else {
                out.push(c);
            }
            upper = false;
        } else {
            upper = true;
        }
    }
    out
}

/// `memberIds` → `member_ids`; Rust keywords become raw identifiers.
fn field_name(key: &str) -> String {
    let mut out = String::new();
    let mut prev_lower = false;
    for c in key.chars() {
        if c.is_ascii_uppercase() {
            if prev_lower {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
            prev_lower = false;
        } else if c.is_ascii_alphanumeric() {
            out.push(c);
            prev_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        } else {
            out.push('_');
            prev_lower = false;
        }
    }
    match out.as_str() {
        "type" | "match" | "ref" | "move" | "self" | "struct" | "enum" | "mod" | "use" | "in"
        | "loop" | "box" | "where" | "async" | "await" | "dyn" | "fn" | "impl" | "trait"
        | "crate" | "super" | "static" | "const" | "let" | "mut" | "pub" | "return" | "as"
        | "for" | "if" | "else" | "while" | "true" | "false" | "extern" | "unsafe" | "override"
        | "final" | "macro" | "priv" | "virtual" | "yield" | "try" | "gen" => format!("r#{out}"),
        _ => out,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        assert_eq!(field_name("memberIds"), "member_ids");
        assert_eq!(field_name("type"), "r#type");
        assert_eq!(field_name("messageLineBytes"), "message_line_bytes");
        assert_eq!(pascal("session.ready"), "SessionReady");
    }
}
