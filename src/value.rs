//! The partial value tree, and the two state axes a path carries.

/// How far along a value is, syntactically.
///
/// Orthogonal to validation state (see `Validation`, added in the validation
/// layer). `Missing` is distinct from `Incomplete`: a key that has not appeared
/// at all is missing, whereas one whose value is half-written is incomplete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// No value at this path (yet).
    Missing,
    /// Present but still being written.
    Incomplete,
    /// Finished. Per the stability guarantee, its value can no longer change.
    Complete,
}

/// A JSON number, stored as its original lexeme so no precision is lost on the
/// way through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Number(String);

impl Number {
    pub(crate) fn new(text: String) -> Self {
        Number(text)
    }
    /// The literal text as it appeared in the input.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn as_f64(&self) -> Option<f64> {
        self.0.parse().ok()
    }
    pub fn as_i64(&self) -> Option<i64> {
        self.0.parse().ok()
    }
}

impl std::fmt::Display for Number {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// A value in a partially-parsed document.
///
/// The `Partial*` variants exist so a snapshot is honest about what is still in
/// flight. A `PartialString` carries only the **decoded-stable prefix** — the
/// part that cannot change (see the stability guarantee) — so a trailing
/// backslash or a half-written `\uXXXX` contributes nothing until it resolves.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
    /// A string still open. Payload is the decoded-stable prefix.
    PartialString(String),
    /// A number whose end has not been seen. Its value may still change —
    /// `10` can still become `100` — so it is never reported `Complete`.
    PartialNumber(String),
    /// A literal still being spelled (`tru`). Payload is what has arrived.
    PartialLiteral(String),
}

impl Value {
    /// Whether this node is finished. Containers report on themselves only —
    /// a closed object is complete even while the document continues.
    pub fn is_complete(&self) -> bool {
        !matches!(
            self,
            Value::PartialString(_) | Value::PartialNumber(_) | Value::PartialLiteral(_)
        )
    }

    /// Look up a child by one JSON Pointer token: an object member name, or
    /// an array index in decimal.
    pub fn child(&self, token: &str) -> Option<&Value> {
        match self {
            Value::Object(members) => members.iter().find(|(k, _)| k == token).map(|(_, v)| v),
            Value::Array(items) => token.parse::<usize>().ok().and_then(|i| items.get(i)),
            _ => None,
        }
    }
}
