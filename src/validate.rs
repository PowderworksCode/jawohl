//! Incremental validation: deciding what a **prefix** already proves.
//!
//! An ordinary validator answers "is this document valid?" once the document
//! exists. This one answers a harder question continuously: given only what has
//! arrived, is this constraint already decided? That is what lets a caller
//! cancel a generation that cannot possibly succeed, instead of paying for it
//! and discovering the failure at the end.
//!
//! The answer is driven by the monotonicity of each constraint, classified when
//! the schema was compiled — see [`crate::schema`].

use crate::schema::{AdditionalProperties, EnumTrie, Node, NodeId, Pattern, Schema, TypeSet};
use crate::value::Value;

/// What is known about a value's validity.
///
/// The two "so far" states are what make this different from a batch
/// validator, and [`IrrecoverablyInvalid`](Validation::IrrecoverablyInvalid) is
/// the one that pays for itself: it means no continuation of the input can make
/// this value valid, so the caller may stop now.
#[derive(jedem::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    /// Nothing decided yet — the value is still too incomplete to judge.
    Pending,
    /// Every constraint that can be judged so far holds, but the value is not
    /// finished.
    ValidSoFar,
    /// The value is complete and every constraint holds.
    Valid,
    /// The value is complete and some constraint fails.
    Invalid,
    /// Some constraint fails and **no continuation can repair it**. The whole
    /// point of the exercise.
    IrrecoverablyInvalid,
}

impl Validation {
    /// Fold constraint outcomes into one state, per the aggregation rule:
    /// irrecoverable wins, then invalid, then pending, else valid-so-far.
    fn fold(parts: impl IntoIterator<Item = Validation>) -> Validation {
        let mut seen_invalid = false;
        let mut seen_pending = false;
        for p in parts {
            match p {
                Validation::IrrecoverablyInvalid => return Validation::IrrecoverablyInvalid,
                Validation::Invalid => seen_invalid = true,
                Validation::Pending => seen_pending = true,
                Validation::ValidSoFar | Validation::Valid => {}
            }
        }
        if seen_invalid {
            Validation::Invalid
        } else if seen_pending {
            Validation::Pending
        } else {
            Validation::ValidSoFar
        }
    }

    /// Is this a settled failure?
    pub fn is_failure(self) -> bool {
        matches!(self, Validation::Invalid | Validation::IrrecoverablyInvalid)
    }

    /// Can no further input rescue this?
    pub fn is_irrecoverable(self) -> bool {
        self == Validation::IrrecoverablyInvalid
    }
}

/// How numeric prefixes are read.
///
/// The sharpest problem in the design. `"limit": 1000` against `maximum: 100`
/// looks like an obvious early cancel — but `1000` may still become `1000e-9`,
/// which is `0.000001`, and the `e` need not come next (`1000.5e-9` is legal
/// too). Under exact analysis **no numeric bound is ever decidable before the
/// number is delimited**, which deletes the feature entirely.
///
/// So the assumption is made explicit, and enforced.
#[derive(jedem::Enum, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumberProfile {
    /// Assume plain decimal — no `e`/`E` exponent. A prefix then bounds an
    /// interval (`1000` ⇒ `[1000, 1001)`) and bounds decide early.
    ///
    /// **Enforced, not assumed:** if an exponent does appear, the stream fails
    /// with [`ParseErrorKind::NumberProfileViolated`] rather than quietly
    /// re-widening and pretending the earlier verdict was fine. Either the
    /// guarantee held, or the caller is told it did not.
    ///
    /// [`ParseErrorKind::NumberProfileViolated`]: crate::ParseErrorKind::NumberProfileViolated
    #[default]
    PlainDecimal,
    /// Sound for all of JSON. Numeric bounds resolve only once the number is
    /// delimited, and no exponent is ever a violation.
    Exact,
}

/// The interval a numeric prefix could still land in, under a profile.
///
/// Returns `None` when nothing can be said — under [`NumberProfile::Exact`],
/// or for a prefix that is not yet a number at all.
fn numeric_interval(prefix: &str, profile: NumberProfile) -> Option<(f64, f64)> {
    if profile == NumberProfile::Exact {
        return None;
    }
    if prefix.contains(['e', 'E']) {
        // A violation the caller is told about elsewhere; say nothing here.
        return None;
    }
    let negative = prefix.starts_with('-');
    let digits = prefix.trim_start_matches('-');
    if digits.is_empty() {
        return None;
    }
    // Under plain-decimal, appending digits only refines downward-then-upward
    // within one unit of the last integral place: `1000` covers [1000, 1001).
    let base: f64 = digits.parse().ok()?;
    let (lo, hi) = if digits.contains('.') {
        // Fractional digits only add magnitude in the same direction.
        let places = digits.split('.').nth(1).map(str::len).unwrap_or(0);
        let step = 10f64.powi(-(places as i32));
        (base, base + step)
    } else {
        // An integer prefix may still gain digits, so the upper bound is open.
        (base, f64::INFINITY)
    };
    Some(if negative { (-hi, -lo) } else { (lo, hi) })
}

/// A validator bound to one schema.
#[derive(Debug, Clone)]
pub struct Validator {
    schema: Schema,
    profile: NumberProfile,
}

impl Validator {
    pub fn new(schema: Schema, profile: NumberProfile) -> Self {
        Validator { schema, profile }
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    #[allow(dead_code)]
    pub fn profile(&self) -> NumberProfile {
        self.profile
    }

    /// Judge `value` against the schema node `node`, knowing whether the value
    /// is syntactically finished.
    pub fn judge(&self, node: NodeId, value: &Value, complete: bool) -> Validation {
        let n = self.schema.node(node);
        if let Some(b) = n.boolean {
            return if b {
                if complete {
                    Validation::Valid
                } else {
                    Validation::ValidSoFar
                }
            } else {
                Validation::IrrecoverablyInvalid
            };
        }

        let folded = Validation::fold([
            self.check_type(n, value, complete),
            self.check_enum_and_const(n, value, complete),
            self.check_string(n, value, complete),
            self.check_number(n, value, complete),
            self.check_array(n, value, complete),
            self.check_object(n, value, complete),
            self.check_combinators(n, value, complete),
        ]);
        // `Valid` is only reachable once the value is whole.
        match (folded, complete) {
            (Validation::ValidSoFar, true) => Validation::Valid,
            (other, _) => other,
        }
    }

    /// The kind of a value, as a one-element type set — `None` when the value
    /// is too partial to know (it never is: the first byte fixes the kind).
    fn kind_of(value: &Value) -> TypeSet {
        match value {
            Value::Null => TypeSet::NULL,
            Value::Bool(_) => TypeSet::BOOLEAN,
            Value::Object(_) => TypeSet::OBJECT,
            Value::Array(_) => TypeSet::ARRAY,
            Value::String(_) | Value::PartialString(_) => TypeSet::STRING,
            Value::Number(_) | Value::PartialNumber(_) => TypeSet::NUMBER.union(TypeSet::INTEGER),
            // A half-written literal is `true`/`false`/`null` — either way a
            // scalar whose kind is not yet pinned down.
            Value::PartialLiteral(_) => TypeSet::BOOLEAN.union(TypeSet::NULL),
        }
    }

    /// `type` is the earliest possible verdict: the first byte of a value
    /// fixes its kind, so a mismatch is irrecoverable immediately.
    fn check_type(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let Some(want) = n.types else {
            return Validation::ValidSoFar;
        };
        let got = Self::kind_of(value);
        if !want.contains(got) {
            return Validation::IrrecoverablyInvalid;
        }
        // `integer` is only decidable once the number is whole.
        if complete && !want.contains(TypeSet::NUMBER) && want.contains(TypeSet::INTEGER) {
            if let Value::Number(num) = value {
                let is_integral = num.as_f64().map(|f| f.fract() == 0.0).unwrap_or(false);
                if !is_integral {
                    return Validation::Invalid;
                }
            }
        }
        Validation::ValidSoFar
    }

    fn check_enum_and_const(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let mut parts = Vec::new();
        if let Some(t) = &n.enumeration {
            parts.push(check_enum(t, value, complete));
        }
        if let Some(c) = &n.const_value {
            parts.push(check_const(c, value, complete));
        }
        Validation::fold(parts)
    }

    fn check_string(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let (text, settled) = match value {
            Value::String(s) => (s.as_str(), true),
            Value::PartialString(s) => (s.as_str(), false),
            _ => return Validation::ValidSoFar,
        };
        let chars = text.chars().count() as u64;
        let mut parts = Vec::new();

        // maxLength seals shut: a string only grows.
        if let Some(max) = n.max_length {
            if chars > max {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        // minLength seals open: once reached it stays reached.
        if let Some(min) = n.min_length {
            parts.push(if chars >= min {
                Validation::ValidSoFar
            } else if settled && complete {
                Validation::Invalid
            } else {
                Validation::Pending
            });
        }
        if let Some(p) = &n.pattern {
            parts.push(check_pattern(p, text, settled && complete));
        }
        Validation::fold(parts)
    }

    fn check_number(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let (text, settled) = match value {
            Value::Number(num) => (num.as_str(), true),
            Value::PartialNumber(t) => (t.as_str(), false),
            _ => return Validation::ValidSoFar,
        };
        let has_bounds = n.minimum.is_some()
            || n.maximum.is_some()
            || n.exclusive_minimum.is_some()
            || n.exclusive_maximum.is_some();
        if !has_bounds && n.multiple_of.is_none() {
            return Validation::ValidSoFar;
        }

        if settled && complete {
            let Some(v) = text.parse::<f64>().ok() else {
                return Validation::Invalid;
            };
            let mut parts = Vec::new();
            if let Some(m) = n.minimum {
                parts.push(ok_or_invalid(v >= m));
            }
            if let Some(m) = n.maximum {
                parts.push(ok_or_invalid(v <= m));
            }
            if let Some(m) = n.exclusive_minimum {
                parts.push(ok_or_invalid(v > m));
            }
            if let Some(m) = n.exclusive_maximum {
                parts.push(ok_or_invalid(v < m));
            }
            if let Some(m) = n.multiple_of {
                parts.push(ok_or_invalid(m != 0.0 && (v / m).fract().abs() < 1e-9));
            }
            return Validation::fold(parts);
        }

        // Partial number: only bounds can speak, and only under a profile that
        // makes the completion set an interval.
        let Some((lo, hi)) = numeric_interval(text, self.profile) else {
            return Validation::Pending;
        };
        let mut parts = Vec::new();
        // The whole interval is out of range -> nothing can rescue it.
        if let Some(m) = n.maximum {
            if lo > m {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        if let Some(m) = n.exclusive_maximum {
            if lo >= m {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        if let Some(m) = n.minimum {
            if hi < m {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        if let Some(m) = n.exclusive_minimum {
            if hi <= m {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        parts.push(Validation::Pending);
        Validation::fold(parts)
    }

    fn check_array(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let Value::Array(items) = value else {
            return Validation::ValidSoFar;
        };
        let len = items.len() as u64;
        let mut parts = Vec::new();
        // Counts only grow, so a max seals shut and a min seals open.
        if let Some(max) = n.max_items {
            if len > max {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        if let Some(min) = n.min_items {
            parts.push(if len >= min {
                Validation::ValidSoFar
            } else if complete {
                Validation::Invalid
            } else {
                Validation::Pending
            });
        }
        if n.unique_items {
            // A duplicate is permanent.
            let mut seen: Vec<&Value> = Vec::new();
            let mut dup = false;
            for it in items {
                if seen.contains(&it) {
                    dup = true;
                    break;
                }
                seen.push(it);
            }
            if dup {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        // Element schemas.
        for (i, item) in items.iter().enumerate() {
            let child_complete = complete || i + 1 < items.len();
            if let Some(pi) = n.prefix_items.get(i) {
                parts.push(self.judge(*pi, item, child_complete));
            } else if let Some(items_node) = n.items {
                parts.push(self.judge(items_node, item, child_complete));
            }
        }
        Validation::fold(parts)
    }

    fn check_object(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let Value::Object(members) = value else {
            return Validation::ValidSoFar;
        };
        let count = members.len() as u64;
        let mut parts = Vec::new();
        if let Some(max) = n.max_properties {
            if count > max {
                parts.push(Validation::IrrecoverablyInvalid);
            }
        }
        if let Some(min) = n.min_properties {
            parts.push(if count >= min {
                Validation::ValidSoFar
            } else if complete {
                Validation::Invalid
            } else {
                Validation::Pending
            });
        }
        // `required` can only be settled at the closing brace: a key that has
        // not arrived may still arrive.
        for req in &n.required {
            let present = members.iter().any(|(k, _)| k == req);
            parts.push(if present {
                Validation::ValidSoFar
            } else if complete {
                Validation::Invalid
            } else {
                Validation::Pending
            });
        }
        for (i, (key, v)) in members.iter().enumerate() {
            let child_complete = complete || i + 1 < members.len();
            if let Some(child) = n.properties.get(key) {
                parts.push(self.judge(*child, v, child_complete));
            } else {
                match &n.additional_properties {
                    // An unknown key is irrecoverable the moment it completes.
                    AdditionalProperties::Forbidden => parts.push(Validation::IrrecoverablyInvalid),
                    AdditionalProperties::Schema(s) => {
                        parts.push(self.judge(*s, v, child_complete))
                    }
                    AdditionalProperties::Permitted => {}
                }
            }
        }
        Validation::fold(parts)
    }

    /// Combinators.
    ///
    /// `allOf` is an intersection, so it composes incrementally for free: any
    /// dead branch kills the whole. `anyOf`, `oneOf` and `not` need per-branch
    /// state to be judged incrementally, and are deliberately decided **at
    /// completion** in this version — a conservative `Pending` beforehand,
    /// never a guess. See `DESIGN.md` §9.
    fn check_combinators(&self, n: &Node, value: &Value, complete: bool) -> Validation {
        let mut parts = Vec::new();
        for a in &n.all_of {
            parts.push(self.judge(*a, value, complete));
        }
        if !n.any_of.is_empty() {
            parts.push(if !complete {
                Validation::Pending
            } else {
                let any = n
                    .any_of
                    .iter()
                    .any(|b| !self.judge(*b, value, true).is_failure());
                ok_or_invalid(any)
            });
        }
        if !n.one_of.is_empty() {
            parts.push(if !complete {
                Validation::Pending
            } else {
                let hits = n
                    .one_of
                    .iter()
                    .filter(|b| !self.judge(**b, value, true).is_failure())
                    .count();
                ok_or_invalid(hits == 1)
            });
        }
        if let Some(not) = n.not {
            parts.push(if !complete {
                Validation::Pending
            } else {
                ok_or_invalid(self.judge(not, value, true).is_failure())
            });
        }
        Validation::fold(parts)
    }

    /// Walk from the root to the schema node governing `pointer_tokens`.
    /// `None` when the schema says nothing about that path.
    pub fn node_for(&self, tokens: &[String]) -> Option<NodeId> {
        let mut cur = self.schema.root();
        for tok in tokens {
            let n = self.schema.node(cur);
            let next = if let Ok(i) = tok.parse::<usize>() {
                n.prefix_items.get(i).copied().or(n.items)
            } else {
                n.properties
                    .get(tok)
                    .copied()
                    .or(match &n.additional_properties {
                        AdditionalProperties::Schema(s) => Some(*s),
                        _ => None,
                    })
            };
            cur = next?;
        }
        Some(cur)
    }
}

fn ok_or_invalid(ok: bool) -> Validation {
    if ok {
        Validation::ValidSoFar
    } else {
        Validation::Invalid
    }
}

/// `enum` is exactly decidable on a prefix: ask whether any member still has
/// the text so far as a prefix.
fn check_enum(t: &EnumTrie, value: &Value, complete: bool) -> Validation {
    match value {
        Value::String(s) => {
            if t.contains(s) {
                Validation::ValidSoFar
            } else if complete {
                Validation::Invalid
            } else {
                Validation::IrrecoverablyInvalid
            }
        }
        Value::PartialString(s) => {
            if t.prefix_is_live(s) {
                Validation::Pending
            } else {
                Validation::IrrecoverablyInvalid
            }
        }
        // A non-string value can only match a non-string member.
        _ if !t.has_non_strings() => Validation::IrrecoverablyInvalid,
        _ => Validation::Pending,
    }
}

fn check_const(expected: &Value, value: &Value, complete: bool) -> Validation {
    match (expected, value) {
        (Value::String(want), Value::PartialString(got)) => {
            if want.starts_with(got.as_str()) {
                Validation::Pending
            } else {
                Validation::IrrecoverablyInvalid
            }
        }
        (want, got) if complete => ok_or_invalid(want == got),
        _ => Validation::Pending,
    }
}

fn check_pattern(p: &Pattern, text: &str, settled: bool) -> Validation {
    if settled {
        return ok_or_invalid(p.matches(text));
    }
    if p.prefix_is_live(text) {
        Validation::Pending
    } else {
        Validation::IrrecoverablyInvalid
    }
}
