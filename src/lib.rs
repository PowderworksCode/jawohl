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

mod error;
mod event;
mod parser;
mod value;

pub use error::{ParseError, ParseErrorKind};
pub use event::{Event, ValueKind};
pub use parser::DEFAULT_MAX_DEPTH;
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
}

impl Default for Stream {
    fn default() -> Self {
        Self::new()
    }
}

impl Stream {
    pub fn new() -> Self {
        Stream {
            parser: Parser::new(),
        }
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
        self.parser.feed(chunk)
    }

    /// The document as it currently stands, including any value still in
    /// flight (as one of the `Partial*` variants). `None` before the first
    /// non-whitespace byte.
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
pub fn get_closing_string_for_partial_json(input: &str) -> Result<String, ParseError> {
    let completed = complete_json(input)?;
    Ok(completed.strip_prefix(input).unwrap_or("").to_string())
}
