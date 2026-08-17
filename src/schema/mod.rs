//! JSON Schema, compiled into a form shaped for **streaming** evaluation.
//!
//! JSON Schema is the interchange format, not the execution format. Every
//! ecosystem can already describe validity — Pydantic, Zod, DataAnnotations,
//! Jakarta, garde — and all of them can emit JSON Schema. jawohl takes that as
//! input and compiles it into a constraint IR built for one question the
//! ordinary validators never ask:
//!
//! > given only a **prefix** of a value, is this constraint already decided?
//!
//! # The organising idea: monotonicity
//!
//! Constraints differ in how they behave as a value grows, and that difference
//! is the whole design. It is recorded explicitly on every constraint as a
//! [`Monotonicity`], because it is what the evaluator dispatches on:
//!
//! * [`Monotonicity::SealsShut`] — once violated,永 violated. `maxLength` can
//!   only be exceeded further as a string grows, so it supports **early
//!   rejection**, which is what lets a caller cancel generation.
//! * [`Monotonicity::SealsOpen`] — once satisfied, always satisfied.
//!   `minLength` only becomes more true, so it can be reported as
//!   valid-so-far before the value finishes.
//! * [`Monotonicity::AtCompletion`] — decidable only when the value is whole.
//!   `multipleOf` cannot be judged from `1` when the number may become `12`.
//!
//! The classification is not uniform per keyword — it depends on the value's
//! type and, for `pattern` and the numeric bounds, on the constraint itself.
//! See [`Pattern`] for why an unanchored regex can never reject early, and the
//! numeric-bounds discussion in `DESIGN.md` §4.2 for why exponent notation
//! makes `maximum` undecidable on a prefix in the general case.
//!
//! # Honesty
//!
//! Anything that cannot be lowered is recorded in the [`LoweringReport`] and
//! reported to the caller. A constraint jawohl silently ignored would be worse
//! than one it refused: the caller would believe a value had been checked when
//! it had not.

mod pattern;
mod trie;

pub use pattern::Pattern;
pub use trie::EnumTrie;

use crate::value::Value;
use std::collections::BTreeMap;

/// How a constraint behaves as the value it constrains grows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Monotonicity {
    /// Once violated it can never be repaired — supports early rejection.
    SealsShut,
    /// Once satisfied it can never be unsatisfied — supports early
    /// valid-so-far.
    SealsOpen,
    /// Only decidable once the value is complete.
    AtCompletion,
}

/// The JSON types a value may take, as a set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TypeSet(u8);

impl TypeSet {
    pub const NULL: TypeSet = TypeSet(1 << 0);
    pub const BOOLEAN: TypeSet = TypeSet(1 << 1);
    pub const OBJECT: TypeSet = TypeSet(1 << 2);
    pub const ARRAY: TypeSet = TypeSet(1 << 3);
    pub const NUMBER: TypeSet = TypeSet(1 << 4);
    pub const INTEGER: TypeSet = TypeSet(1 << 5);
    pub const STRING: TypeSet = TypeSet(1 << 6);

    pub fn from_name(name: &str) -> Option<TypeSet> {
        Some(match name {
            "null" => Self::NULL,
            "boolean" => Self::BOOLEAN,
            "object" => Self::OBJECT,
            "array" => Self::ARRAY,
            // an integer is a number, so `number` admits everything `integer`
            // does; the narrowing is applied at completion.
            "number" => TypeSet(Self::NUMBER.0 | Self::INTEGER.0),
            "integer" => Self::INTEGER,
            "string" => Self::STRING,
            _ => return None,
        })
    }

    pub fn contains(self, other: TypeSet) -> bool {
        self.0 & other.0 != 0
    }

    pub fn union(self, other: TypeSet) -> TypeSet {
        TypeSet(self.0 | other.0)
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// What `additionalProperties` says.
#[derive(Debug, Clone, Default)]
pub enum AdditionalProperties {
    /// Absent, or `true`: anything goes.
    #[default]
    Permitted,
    /// `false`: an unknown key makes the object irrecoverably invalid the
    /// moment that key completes.
    Forbidden,
    /// A schema every unknown key's value must satisfy.
    Schema(NodeId),
}

/// An index into [`Schema`]'s arena.
///
/// The IR is an arena of nodes rather than a tree of boxes because `$ref` makes
/// a schema a **graph**: `{"$ref": "#"}` is a cycle, and Pydantic emits `$defs`
/// plus `$ref` for any nested model, so recursion is the common case rather
/// than an exotic one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId(pub(crate) u32);

/// One schema node.
#[derive(Debug, Clone, Default)]
pub struct Node {
    /// `true` / `false` schemas, which accept everything or nothing.
    pub boolean: Option<bool>,
    pub types: Option<TypeSet>,

    pub min_length: Option<u64>,
    pub max_length: Option<u64>,
    pub pattern: Option<Pattern>,

    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
    pub exclusive_minimum: Option<f64>,
    pub exclusive_maximum: Option<f64>,
    pub multiple_of: Option<f64>,

    pub min_items: Option<u64>,
    pub max_items: Option<u64>,
    pub unique_items: bool,
    pub items: Option<NodeId>,
    pub prefix_items: Vec<NodeId>,

    pub properties: BTreeMap<String, NodeId>,
    pub required: Vec<String>,
    pub additional_properties: AdditionalProperties,
    pub min_properties: Option<u64>,
    pub max_properties: Option<u64>,

    pub enumeration: Option<EnumTrie>,
    pub const_value: Option<Value>,

    pub all_of: Vec<NodeId>,
    pub any_of: Vec<NodeId>,
    pub one_of: Vec<NodeId>,
    pub not: Option<NodeId>,
}

impl Node {
    /// A node that accepts anything.
    fn permissive() -> Node {
        Node::default()
    }

    /// How the `maximum`/`minimum` family behaves on a prefix.
    ///
    /// [`Monotonicity::AtCompletion`] in the exact reading, because exponent
    /// notation can rescue any numeric prefix: `1000` may still become
    /// `1000e-9`. The evaluator may sharpen this under a declared
    /// [`crate::NumberProfile`]; the IR records only what is true of JSON
    /// itself.
    pub fn numeric_bound_monotonicity(&self) -> Monotonicity {
        Monotonicity::AtCompletion
    }
}

/// What the compiler could and could not lower.
///
/// Surfaced to the caller because silence is the failure mode this design
/// exists to avoid: a constraint that was quietly dropped looks exactly like a
/// constraint that passed.
#[derive(Debug, Clone, Default)]
pub struct LoweringReport {
    /// Constraints successfully compiled.
    pub compiled: usize,
    /// Keywords present in the schema that jawohl did not lower.
    pub unsupported: Vec<Unsupported>,
}

/// One keyword that was not lowered, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// JSON Pointer to the keyword within the schema document.
    pub location: String,
    pub keyword: String,
    pub reason: String,
}

impl std::fmt::Display for LoweringReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "{} constraints compiled", self.compiled)?;
        if self.unsupported.is_empty() {
            write!(f, "0 unsupported")
        } else {
            writeln!(f, "{} unsupported:", self.unsupported.len())?;
            for u in &self.unsupported {
                writeln!(f, "  {} at {}: {}", u.keyword, u.location, u.reason)?;
            }
            Ok(())
        }
    }
}

/// A compiled schema.
#[derive(Debug, Clone)]
pub struct Schema {
    nodes: Vec<Node>,
    root: NodeId,
    report: LoweringReport,
}

impl Schema {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.0 as usize]
    }

    pub fn root(&self) -> NodeId {
        self.root
    }

    pub fn root_node(&self) -> &Node {
        self.node(self.root)
    }

    /// What was and was not lowered. Check it: a schema that compiled with
    /// unsupported keywords is validating less than the caller wrote.
    pub fn lowering_report(&self) -> &LoweringReport {
        &self.report
    }

    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }
}

mod compile;
pub use compile::{compile, SchemaError};
