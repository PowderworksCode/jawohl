//! Lowering a JSON Schema document into the constraint IR.
//!
//! The schema document is itself parsed with jawohl's own parser — the one
//! piece of dogfooding available for free, and the reason the default build
//! still has no JSON dependency.
//!
//! Target draft: **2020-12**. Keywords outside the supported set are recorded
//! in the [`LoweringReport`] rather than ignored.

use super::{
    AdditionalProperties, EnumTrie, LoweringReport, Node, NodeId, Pattern, Schema, TypeSet,
    Unsupported,
};
use crate::value::Value;
use std::collections::BTreeMap;

/// Why a schema could not be compiled at all (as distinct from a keyword that
/// merely could not be lowered, which is reported and survivable).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaError {
    /// The document is not valid JSON.
    NotJson(crate::ParseError),
    /// The document parsed but is not an object or a boolean.
    NotASchema,
    /// A `$ref` that does not resolve within this document.
    UnresolvableRef(String),
}

impl std::fmt::Display for SchemaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SchemaError::NotJson(e) => write!(f, "schema is not valid JSON: {e}"),
            SchemaError::NotASchema => {
                write!(f, "a schema must be an object or a boolean")
            }
            SchemaError::UnresolvableRef(r) => write!(f, "cannot resolve $ref {r:?}"),
        }
    }
}

impl std::error::Error for SchemaError {}

/// Keywords understood at completion but not usable for a partial value, and
/// keywords outside the supported subset. Recorded, never silently dropped.
const DEFERRED: &[(&str, &str)] = &[
    (
        "unevaluatedProperties",
        "requires whole-document annotation results; not evaluable incrementally",
    ),
    (
        "unevaluatedItems",
        "requires whole-document annotation results; not evaluable incrementally",
    ),
    ("if", "conditional application is not lowered yet"),
    ("then", "conditional application is not lowered yet"),
    ("else", "conditional application is not lowered yet"),
    ("dependentSchemas", "not lowered yet"),
    ("dependentRequired", "not lowered yet"),
    ("contains", "not lowered yet"),
    ("propertyNames", "not lowered yet"),
    ("patternProperties", "not lowered yet"),
    ("format", "annotation only in 2020-12; not asserted"),
];

/// Compile a JSON Schema document.
///
/// ```
/// # use jawohl::schema::compile;
/// let s = compile(r#"{"type":"string","minLength":3,"maxLength":20}"#).unwrap();
/// assert_eq!(s.lowering_report().compiled, 3);
/// assert!(s.lowering_report().unsupported.is_empty());
/// ```
pub fn compile(document: &str) -> Result<Schema, SchemaError> {
    let value = crate::parse_complete(document).map_err(SchemaError::NotJson)?;
    let mut c = Compiler {
        nodes: Vec::new(),
        report: LoweringReport::default(),
        root_value: value.clone(),
        in_progress: BTreeMap::new(),
    };
    let root = c.lower(&value, "")?;
    Ok(Schema {
        nodes: c.nodes,
        root,
        report: c.report,
    })
}

struct Compiler {
    nodes: Vec<Node>,
    report: LoweringReport,
    /// Kept so `$ref` can be resolved against the document it came from.
    root_value: Value,
    /// Pointers already being lowered, so a recursive `$ref` reuses the node
    /// under construction instead of expanding forever.
    in_progress: BTreeMap<String, NodeId>,
}

impl Compiler {
    fn alloc(&mut self, node: Node) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(node);
        id
    }

    fn reserve(&mut self) -> NodeId {
        self.alloc(Node::default())
    }

    fn note(&mut self, location: &str, keyword: &str, reason: &str) {
        self.report.unsupported.push(Unsupported {
            location: location.to_string(),
            keyword: keyword.to_string(),
            reason: reason.to_string(),
        });
    }

    fn lower(&mut self, value: &Value, at: &str) -> Result<NodeId, SchemaError> {
        // `true` and `false` are valid schemas in their own right.
        if let Value::Bool(b) = value {
            let mut n = Node::permissive();
            n.boolean = Some(*b);
            self.report.compiled += 1;
            return Ok(self.alloc(n));
        }
        let Value::Object(members) = value else {
            return Err(SchemaError::NotASchema);
        };

        // A `$ref` replaces the node it appears in (2020-12 allows siblings,
        // but the common emitters do not use them, and honouring only the ref
        // is the conservative reading).
        if let Some(Value::String(target)) = get(members, "$ref") {
            return self.resolve_ref(target, at);
        }

        let id = self.reserve();
        self.in_progress.insert(at.to_string(), id);
        let mut n = Node::default();

        for (key, v) in members {
            let loc = format!("{at}/{key}");
            match key.as_str() {
                "type" => {
                    let mut set = TypeSet::default();
                    match v {
                        Value::String(s) => match TypeSet::from_name(s) {
                            Some(t) => set = set.union(t),
                            None => self.note(&loc, "type", &format!("unknown type {s:?}")),
                        },
                        Value::Array(items) => {
                            for it in items {
                                if let Value::String(s) = it {
                                    match TypeSet::from_name(s) {
                                        Some(t) => set = set.union(t),
                                        None => {
                                            self.note(&loc, "type", &format!("unknown type {s:?}"))
                                        }
                                    }
                                }
                            }
                        }
                        _ => self.note(&loc, "type", "expected a string or array of strings"),
                    }
                    if !set.is_empty() {
                        n.types = Some(set);
                        self.report.compiled += 1;
                    }
                }
                "minLength" => n.min_length = self.u64_at(v, &loc, key, &mut n.types),
                "maxLength" => n.max_length = self.u64_at(v, &loc, key, &mut n.types),
                "minItems" => n.min_items = self.u64_at(v, &loc, key, &mut n.types),
                "maxItems" => n.max_items = self.u64_at(v, &loc, key, &mut n.types),
                "minProperties" => n.min_properties = self.u64_at(v, &loc, key, &mut n.types),
                "maxProperties" => n.max_properties = self.u64_at(v, &loc, key, &mut n.types),
                "minimum" => n.minimum = self.f64_at(v, &loc, key),
                "maximum" => n.maximum = self.f64_at(v, &loc, key),
                "exclusiveMinimum" => n.exclusive_minimum = self.f64_at(v, &loc, key),
                "exclusiveMaximum" => n.exclusive_maximum = self.f64_at(v, &loc, key),
                "multipleOf" => n.multiple_of = self.f64_at(v, &loc, key),
                "uniqueItems" => {
                    if let Value::Bool(b) = v {
                        n.unique_items = *b;
                        self.report.compiled += 1;
                    }
                }
                "pattern" => {
                    if let Value::String(p) = v {
                        match Pattern::compile(p) {
                            Some(pat) => {
                                if !pat.supports_early_rejection() {
                                    self.note(
                                        &loc,
                                        "pattern",
                                        "unanchored: checked at completion, but no partial \
                                         value can be rejected early (any prefix can still be \
                                         extended into a match)",
                                    );
                                }
                                n.pattern = Some(pat);
                                self.report.compiled += 1;
                            }
                            None => self.note(
                                &loc,
                                "pattern",
                                if cfg!(feature = "pattern") {
                                    "regex did not compile"
                                } else {
                                    "the `pattern` feature is disabled; this pattern is not checked"
                                },
                            ),
                        }
                    }
                }
                "enum" => {
                    if let Value::Array(items) = v {
                        let mut strings = Vec::new();
                        let mut other = false;
                        for it in items {
                            match it {
                                Value::String(s) => strings.push(s.clone()),
                                _ => other = true,
                            }
                        }
                        n.enumeration = Some(EnumTrie::new(strings, other));
                        self.report.compiled += 1;
                    }
                }
                "const" => {
                    n.const_value = Some(v.clone());
                    self.report.compiled += 1;
                }
                "required" => {
                    if let Value::Array(items) = v {
                        n.required = items
                            .iter()
                            .filter_map(|i| match i {
                                Value::String(s) => Some(s.clone()),
                                _ => None,
                            })
                            .collect();
                        self.report.compiled += 1;
                    }
                }
                "properties" => {
                    if let Value::Object(props) = v {
                        for (name, sub) in props {
                            let child = self.lower(sub, &format!("{loc}/{name}"))?;
                            n.properties.insert(name.clone(), child);
                        }
                        self.report.compiled += 1;
                    }
                }
                "additionalProperties" => {
                    n.additional_properties = match v {
                        Value::Bool(true) => AdditionalProperties::Permitted,
                        Value::Bool(false) => AdditionalProperties::Forbidden,
                        other => AdditionalProperties::Schema(self.lower(other, &loc)?),
                    };
                    self.report.compiled += 1;
                }
                "items" => {
                    n.items = Some(self.lower(v, &loc)?);
                    self.report.compiled += 1;
                }
                "prefixItems" => {
                    if let Value::Array(items) = v {
                        for (i, sub) in items.iter().enumerate() {
                            let child = self.lower(sub, &format!("{loc}/{i}"))?;
                            n.prefix_items.push(child);
                        }
                        self.report.compiled += 1;
                    }
                }
                "allOf" | "anyOf" | "oneOf" => {
                    if let Value::Array(items) = v {
                        let mut ids = Vec::new();
                        for (i, sub) in items.iter().enumerate() {
                            ids.push(self.lower(sub, &format!("{loc}/{i}"))?);
                        }
                        match key.as_str() {
                            "allOf" => n.all_of = ids,
                            "anyOf" => n.any_of = ids,
                            _ => n.one_of = ids,
                        }
                        self.report.compiled += 1;
                    }
                }
                "not" => {
                    n.not = Some(self.lower(v, &loc)?);
                    self.report.compiled += 1;
                }
                // Definition containers hold subschemas that only matter when
                // referenced; lowering them eagerly would double-count.
                "$defs" | "definitions" => {}
                // Annotations, carrying no assertion.
                "title" | "description" | "default" | "examples" | "$schema" | "$id"
                | "$comment" | "deprecated" | "readOnly" | "writeOnly" => {}
                other => {
                    if let Some((_, why)) = DEFERRED.iter().find(|(k, _)| *k == other) {
                        self.note(&loc, other, why);
                    } else {
                        self.note(&loc, other, "unrecognised keyword");
                    }
                }
            }
        }

        self.in_progress.remove(at);
        self.nodes[id.0 as usize] = n;
        Ok(id)
    }

    /// Resolve a local `$ref`. Remote references are refused rather than
    /// fetched — jawohl does no I/O.
    fn resolve_ref(&mut self, target: &str, at: &str) -> Result<NodeId, SchemaError> {
        if !target.starts_with('#') {
            self.note(at, "$ref", "only local references (`#/...`) are supported");
            return Err(SchemaError::UnresolvableRef(target.to_string()));
        }
        let pointer = &target[1..];
        // A reference back to something already being lowered is a cycle; hand
        // back the node under construction so recursion terminates.
        if let Some(id) = self.in_progress.get(pointer) {
            return Ok(*id);
        }
        let Some(target_value) = pointer_into(&self.root_value, pointer) else {
            return Err(SchemaError::UnresolvableRef(target.to_string()));
        };
        let target_value = target_value.clone();
        self.report.compiled += 1;
        self.lower(&target_value, pointer)
    }

    fn u64_at(
        &mut self,
        v: &Value,
        loc: &str,
        key: &str,
        _types: &mut Option<TypeSet>,
    ) -> Option<u64> {
        match v {
            Value::Number(n) => match n.as_str().parse::<u64>() {
                Ok(x) => {
                    self.report.compiled += 1;
                    Some(x)
                }
                Err(_) => {
                    self.note(loc, key, "expected a non-negative integer");
                    None
                }
            },
            _ => {
                self.note(loc, key, "expected a number");
                None
            }
        }
    }

    fn f64_at(&mut self, v: &Value, loc: &str, key: &str) -> Option<f64> {
        match v {
            Value::Number(n) => match n.as_f64() {
                Some(x) => {
                    self.report.compiled += 1;
                    Some(x)
                }
                None => {
                    self.note(loc, key, "not representable as a number");
                    None
                }
            },
            _ => {
                self.note(loc, key, "expected a number");
                None
            }
        }
    }
}

fn get<'a>(members: &'a [(String, Value)], key: &str) -> Option<&'a Value> {
    members.iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Walk an RFC 6901 pointer into a value.
fn pointer_into<'a>(root: &'a Value, pointer: &str) -> Option<&'a Value> {
    if pointer.is_empty() {
        return Some(root);
    }
    let mut cur = root;
    for token in pointer.trim_start_matches('/').split('/') {
        let token = token.replace("~1", "/").replace("~0", "~");
        cur = cur.child(&token)?;
    }
    Some(cur)
}
