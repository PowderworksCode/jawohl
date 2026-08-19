//! # jawohl
//!
//! A cross-language incremental parser for streaming structured data — parse
//! JSON while it is still being generated.
//!
//! The premise: structured output from a language model should behave like a
//! progressively materialising typed value, not a blob of text that becomes
//! useful only when generation finishes. Given
//!
//! ```text
//! {"query":"rust par
//! ```
//!
//! jawohl can already tell you that `/query` exists, that its value so far is
//! `rust par`, and that it is not yet final.
//!
//! ## The stability guarantee
//!
//! **Once a path reports [`Syntax::Complete`], no further input can change its
//! value.** That is what makes it safe to act on a value — fire the search,
//! start the tool call — while the rest of the document is still arriving.
//!
//! The guarantee is enforced structurally, and it costs something: a number is
//! complete only once a delimiter proves it ended, because `10` can still
//! become `100`.
//!
//! ## Levels
//!
//! * [`complete_json`] — one-shot: finish a truncated document so it parses.
//! * [`Stream`] — incremental: push chunks, inspect state by JSON Pointer.
//!
//! ```
//! use jawohl::{Stream, Syntax};
//!
//! let mut s = Stream::new();
//! s.push(br#"{"query":"rust par"#).unwrap();
//! assert_eq!(s.status("/query"), Syntax::Incomplete);
//!
//! s.push(br#"ser","limit":10"#).unwrap();
//! assert_eq!(s.status("/query"), Syntax::Complete);   // the closing quote arrived
//! assert_eq!(s.status("/limit"), Syntax::Incomplete); // 10 could still become 100
//! ```

/// The README, compiled and run as doctests.
///
/// The 0.1 README promised wrappers that never shipped and documented behaviour
/// the code did not have. Every Rust snippet in it is now checked by
/// `cargo test`, so it cannot drift from the crate again.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
pub struct Readme;

mod error;
mod event;
mod parser;
pub mod schema;
mod validate;
mod value;

pub use error::{ParseError, ParseErrorKind};
pub use event::{Event, ValueKind};
pub use parser::DEFAULT_MAX_DEPTH;
pub use schema::{compile as compile_schema, LoweringReport, Schema, SchemaError};
pub use validate::{NumberProfile, Validation};
pub use value::{Number, Syntax, Value};

use parser::Parser;

/// An incremental parse in progress.
///
/// Feed it bytes as they arrive with [`push`](Stream::push); ask it what it has
/// with [`snapshot`](Stream::snapshot) and [`status`](Stream::status).
///
/// Chunk boundaries are irrelevant — a chunk may split a `\uXXXX` escape or a
/// multi-byte character and the result is identical to feeding the whole input
/// at once.
pub struct Stream {
    parser: Parser,
    /// Attached by [`Stream::from_json_schema`]. Without one, jawohl parses
    /// and reports structure but judges nothing.
    validator: Option<validate::Validator>,
    /// Last state emitted per path, so a transition is reported once rather
    /// than on every push.
    reported: std::collections::BTreeMap<String, Validation>,
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

#[jedem::export]
impl Stream {
    pub fn new() -> Self {
        Stream {
            parser: Parser::new(),
            validator: None,
            reported: std::collections::BTreeMap::new(),
        }
    }

    /// A stream that validates against a JSON Schema as it parses.
    ///
    /// Uses [`NumberProfile::PlainDecimal`], which is what makes numeric
    /// bounds decidable on a partial value — see
    /// [`with_number_profile`](Stream::with_number_profile) for the trade.
    ///
    /// ```
    /// # use jawohl::{Stream, Validation};
    /// let mut s = Stream::from_json_schema(r#"{"properties":{"role":{"enum":["user","admin"]}}}"#).unwrap();
    /// s.push(br#"{"role":"sup"#).unwrap();
    /// // No member begins "sup", and none ever will.
    /// assert_eq!(s.validation("/role"), Validation::IrrecoverablyInvalid);
    /// assert!(s.is_irrecoverable());
    /// ```
    ///
    /// # Errors
    ///
    /// If the schema is not valid JSON, is not a schema, or contains a `$ref`
    /// that does not resolve. Keywords that merely cannot be *lowered* are not
    /// errors — they are recorded in [`lowering_report`](Stream::lowering_report).
    pub fn from_json_schema(schema: &str) -> Result<Self, SchemaError> {
        let compiled = schema::compile(schema)?;
        let mut s = Stream::new();
        s.validator = Some(validate::Validator::new(compiled, NumberProfile::default()));
        s.parser.set_enforce_plain_decimal(true);
        Ok(s)
    }

    /// Choose how numeric prefixes are read.
    ///
    /// [`NumberProfile::PlainDecimal`] (the default) lets `"limit": 1000` be
    /// rejected against `maximum: 100` before the number is delimited, and
    /// fails the stream if an exponent ever appears. [`NumberProfile::Exact`]
    /// accepts every JSON number and gives up early numeric rejection, because
    /// `1000` may still become `1000e-9`.
    ///
    /// No effect without a schema: with nothing to judge there is nothing to
    /// be unsound about, and `1e10` is simply a number.
    // Consumes `self`, which cannot mean anything once another language owns
    // the handle.
    #[jedem(skip)]
    pub fn with_number_profile(mut self, profile: NumberProfile) -> Self {
        if let Some(v) = self.validator.take() {
            self.validator = Some(validate::Validator::new(v.schema().clone(), profile));
            self.parser
                .set_enforce_plain_decimal(profile == NumberProfile::PlainDecimal);
        }
        self
    }

    /// What the schema compiler could and could not lower. `None` without a
    /// schema.
    ///
    /// Worth checking: a schema that compiled with unsupported keywords is
    /// validating less than its author wrote.
    // Not across a boundary yet: jedem has no lowering for records.
    #[jedem(skip)]
    pub fn lowering_report(&self) -> Option<&LoweringReport> {
        self.validator
            .as_ref()
            .map(|v| v.schema().lowering_report())
    }

    /// How the value at `pointer` stands against the schema.
    ///
    /// [`Validation::Pending`] without a schema, or where the schema says
    /// nothing about that path.
    pub fn validation(&self, pointer: &str) -> Validation {
        let Some(v) = &self.validator else {
            return Validation::Pending;
        };
        let Some(tokens) = parse_pointer(pointer) else {
            return Validation::Pending;
        };
        let Some(node) = v.node_for(&tokens) else {
            return Validation::Pending;
        };
        let Some(snap) = self.parser.snapshot() else {
            return Validation::Pending;
        };
        let mut cur = &snap;
        for tok in &tokens {
            match cur.child(tok) {
                Some(next) => cur = next,
                None => return Validation::Pending,
            }
        }
        v.judge(node, cur, self.status(pointer) == Syntax::Complete)
    }

    /// True when the document can no longer become valid, whatever arrives.
    ///
    /// The early-cancellation signal: a caller seeing this can stop the
    /// generation producing the input rather than paying for the rest of it.
    pub fn is_irrecoverable(&self) -> bool {
        self.validation("").is_irrecoverable()
    }

    /// Set the maximum container nesting depth. The default is
    /// [`DEFAULT_MAX_DEPTH`].
    ///
    /// Nesting deeper than this is a [`ParseErrorKind::DepthLimitExceeded`].
    /// The limit exists because jawohl's input is untrusted model output and
    /// every level costs an allocation: without a ceiling, `[` repeated is a
    /// denial-of-service vector. Raise it if you genuinely have deep
    /// documents; lower it to harden further.
    ///
    /// ```
    /// # use jawohl::Stream;
    /// let mut s = Stream::new().with_max_depth(3);
    /// assert!(s.push(b"[[[[[1]]]]]").is_err());
    /// ```
    // Consumes `self`, which cannot mean anything once another language owns
    // the handle.
    #[jedem(skip)]
    pub fn with_max_depth(mut self, depth: usize) -> Self {
        self.parser.set_max_depth(depth);
        self
    }

    /// Feed the next chunk.
    ///
    /// Returns `Err` only if the input cannot be a prefix of any valid JSON
    /// document. Running out of input mid-value is not an error — it is the
    /// normal state of a stream. Once a stream has failed it stays failed.
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), ParseError> {
        self.parser.feed(chunk)?;
        self.judge_touched_paths();
        Ok(())
    }

    /// Re-judge the paths this push touched, and their ancestors, emitting a
    /// validation event on each transition.
    ///
    /// Validation events mirror parse events rather than being diffed out of
    /// whole snapshots: work is proportional to what actually changed, and a
    /// path whose state has not moved produces no event.
    fn judge_touched_paths(&mut self) {
        if self.validator.is_none() {
            return;
        }
        let touched: Vec<String> = self
            .parser
            .peek_events()
            .iter()
            .filter_map(|e| e.path().map(str::to_string))
            .collect();

        let mut seen: Vec<String> = Vec::new();
        for path in touched {
            // A child's verdict can move its parent's, so walk up to the root.
            let mut p = path.as_str();
            loop {
                if !seen.iter().any(|s| s == p) {
                    seen.push(p.to_string());
                }
                match p.rfind('/') {
                    Some(0) | None => break,
                    Some(i) => p = &p[..i],
                }
            }
            if !seen.iter().any(|s| s.is_empty()) {
                seen.push(String::new());
            }
        }

        for path in seen {
            let now = self.validation(&path);
            let before = self.reported.get(&path).copied();
            if before == Some(now) {
                continue;
            }
            self.reported.insert(path.clone(), now);
            let complete = self.status(&path) == Syntax::Complete;
            if now.is_failure() {
                self.parser.push_event(Event::ValidationFailed {
                    path: path.clone(),
                    state: now,
                });
            } else if complete && now == Validation::Valid {
                self.parser
                    .push_event(Event::ValidationCompleted { path, state: now });
            }
        }
    }

    /// The document as it currently stands, including any value still in
    /// flight (as one of the `Partial*` variants). `None` before the first
    /// non-whitespace byte.
    // Not across a boundary yet: jedem has no lowering for unions.
    #[jedem(skip)]
    pub fn snapshot(&self) -> Option<Value> {
        self.parser.snapshot()
    }

    /// How far along the value at `pointer` is.
    ///
    /// `pointer` is an RFC 6901 JSON Pointer: `""` is the root, `/query` a
    /// member, `/messages/0/role` a nested one.
    pub fn status(&self, pointer: &str) -> Syntax {
        let tokens = match parse_pointer(pointer) {
            Some(t) => t,
            None => return Syntax::Missing,
        };
        let snap = match self.parser.snapshot() {
            Some(s) => s,
            None => return Syntax::Missing,
        };
        let mut cur = &snap;
        for tok in &tokens {
            match cur.child(tok) {
                Some(next) => cur = next,
                None => return Syntax::Missing,
            }
        }
        if !cur.is_complete() {
            return Syntax::Incomplete;
        }
        // A container is complete only once its bracket has closed, which is
        // exactly when it is no longer on the open-frame stack.
        if self.is_open_container(&tokens) {
            Syntax::Incomplete
        } else {
            Syntax::Complete
        }
    }

    /// True once the root value has closed.
    pub fn is_document_complete(&self) -> bool {
        self.parser.document_complete()
    }

    /// The failure that terminated this stream, if any.
    // Not across a boundary yet: jedem has no lowering for records.
    #[jedem(skip)]
    pub fn error(&self) -> Option<&ParseError> {
        self.parser.failure()
    }

    /// Take every change since the last drain.
    ///
    /// The log is append-only and draining empties it, so a consumer that
    /// calls this after each [`push`](Stream::push) sees each event exactly
    /// once. See [`Event`] for the ordering guarantees.
    ///
    /// ```
    /// # use jawohl::{Stream, Event, ValueKind};
    /// let mut s = Stream::new();
    /// s.push(br#"{"a":1}"#).unwrap();
    /// let events = s.changes();
    /// assert!(matches!(events.first(), Some(Event::ValueStarted { kind: ValueKind::Object, .. })));
    /// assert!(matches!(events.last(), Some(Event::DocumentCompleted)));
    /// assert!(s.changes().is_empty()); // drained
    /// ```
    // Not across a boundary yet: jedem has no lowering for unions -- `Event` has struct variants.
    #[jedem(skip)]
    pub fn changes(&mut self) -> Vec<Event> {
        self.parser.drain_events()
    }

    /// Signal end of input: completes a trailing number or literal if it is
    /// well-formed. Containers left open simply stay open.
    pub fn finish(&mut self) -> Result<(), ParseError> {
        self.parser.finish()
    }

    fn is_open_container(&self, tokens: &[String]) -> bool {
        // The open frames spell out a path from the root; any prefix of that
        // path (including the root) names a container still awaiting its
        // closing bracket.
        match self.parser.open_frame_path() {
            Some(open) => tokens.len() <= open.len() && open[..tokens.len()] == *tokens,
            None => false,
        }
    }
}

/// Parse an RFC 6901 JSON Pointer into its tokens.
fn parse_pointer(p: &str) -> Option<Vec<String>> {
    if p.is_empty() {
        return Some(Vec::new());
    }
    if !p.starts_with('/') {
        return None;
    }
    Some(
        p[1..]
            .split('/')
            .map(|t| t.replace("~1", "/").replace("~0", "~"))
            .collect(),
    )
}

/// Parse a complete JSON document.
///
/// A convenience over [`Stream`] for the non-streaming case — and the function
/// jawohl uses on itself to read a JSON Schema, which is why the default build
/// still needs no JSON dependency.
///
/// # Errors
///
/// Returns `Err` if the input is malformed, or if it ends before the document
/// does — an unfinished document is not a complete one.
///
/// ```
/// # use jawohl::{parse_complete, Value};
/// assert_eq!(parse_complete("[1,2]").unwrap(), Value::Array(vec![
///     Value::Number("1".parse().unwrap()),
///     Value::Number("2".parse().unwrap()),
/// ]));
/// assert!(parse_complete("[1,2").is_err());
/// ```
pub fn parse_complete(input: &str) -> Result<Value, ParseError> {
    let mut s = Stream::new();
    s.push(input.as_bytes())?;
    s.finish()?;
    if !s.is_document_complete() {
        return Err(ParseError {
            offset: input.len(),
            kind: ParseErrorKind::UnexpectedEndOfInput,
        });
    }
    s.snapshot().ok_or(ParseError {
        offset: 0,
        kind: ParseErrorKind::UnexpectedEndOfInput,
    })
}

/// Complete a truncated JSON document so that it parses.
///
/// This is jawohl's original entry point, now built on the incremental parser
/// and therefore correct on inputs the bracket-counting version silently
/// corrupted — a dangling `\`, a half-written `\uXXXX`, a partial literal, a
/// key with no value, a trailing comma.
///
/// Anything not yet complete enough to be valid is **dropped**, and anything
/// unambiguously completable is **finished**: `tru` becomes `true`, an open
/// string is closed after its stable prefix.
///
/// ```
/// # use jawohl::complete_json;
/// assert_eq!(
///     complete_json(r#"{"key":"value","arr":[1,2,{"nested":"v"#).unwrap(),
///     r#"{"key":"value","arr":[1,2,{"nested":"v"}]}"#
/// );
/// // a dangling escape is dropped rather than escaping the closing quote
/// assert_eq!(complete_json(r#"{"a":"x\"#).unwrap(), r#"{"a":"x"}"#);
/// // a partial literal is finished
/// assert_eq!(complete_json(r#"{"a":tru"#).unwrap(), r#"{"a":true}"#);
/// ```
///
/// # Errors
///
/// Returns `Err` if the input is not a prefix of any valid JSON document.
/// Unlike 1.0, which returned `Ok` with invalid output, a malformed input is
/// reported as such.
///
/// # A note on numbers
///
/// A trailing undelimited number is emitted as-is (`{"limit":10` completes to
/// `{"limit":10}`), because that is what a display consumer wants. It is
/// therefore the one part of the output that a later chunk may change — use
/// [`Stream::status`] if you need the stability guarantee.
#[jedem::export]
pub fn complete_json(input: &str) -> Result<String, ParseError> {
    let mut s = Stream::new();
    s.push(input.as_bytes())?;
    let plan = s.parser.completion();
    let mut out = String::with_capacity(input.len() + plan.tail.len() + plan.closers.len());
    out.push_str(&input[..plan.keep]);
    out.push_str(&plan.tail);
    out.push_str(&plan.closers);
    Ok(out)
}

/// The closing string 1.0 would have appended — kept for source compatibility.
///
/// Prefer [`complete_json`]: because 2.0 may need to *drop* an incomplete
/// fragment rather than append to it, a suffix alone cannot always express the
/// completion. This returns the suffix when one exists, and an empty string
/// when the completion required dropping something.
#[jedem::export]
pub fn get_closing_string_for_partial_json(input: &str) -> Result<String, ParseError> {
    let completed = complete_json(input)?;
    Ok(completed.strip_prefix(input).unwrap_or("").to_string())
}

// ---------------------------------------------------------------------------

// jawohl's binding surface.
//
// Everything named here is annotated where it is defined — `Stream` and the
// two free functions above, `Syntax` and `Validation` in their own modules.
// There is no separate surface crate restating the API, and so nothing that
// can fall out of step with it.
//
// `Stream` crosses as a handle, which means Python and TypeScript get the
// incremental parser itself rather than a batch-shaped stand-in that
// re-parses from byte zero on every call.
//
// `bindings:` puts generation in the test suite: `cargo test` fails if the
// committed bindings no longer match this surface, and `JEDEM_WRITE=1 cargo
// test` rewrites them.
jedem::surface! {
    name: "jawohl",
    version: "0.2.0",
    api: [Stream, complete_json, get_closing_string_for_partial_json],
    bindings: "bindings",
}
