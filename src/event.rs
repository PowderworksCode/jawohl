//! The event log.
//!
//! The core emits an append-only sequence of changes; consumers drain it with
//! [`Stream::changes`](crate::Stream::changes). It is deliberately **not** a
//! diff over snapshots: a consumer wants to know *what happened*, and
//! re-deriving that by comparing whole documents would cost more than the
//! parse itself — and, once these events cross a language boundary, would make
//! a snapshot per token the dominant expense.
//!
//! # Ordering guarantees
//!
//! These are promises, not incidental behaviour; handlers cannot be written
//! correctly without them.
//!
//! 1. **A path's events are totally ordered** — [`ValueStarted`] before any
//!    [`ValueProgressed`], and both before [`ValueCompleted`].
//! 2. **A child's `ValueCompleted` precedes its parent's.** An object is only
//!    complete once everything inside it is.
//! 3. **`DocumentCompleted` is last, and is emitted exactly once.**
//!
//! [`ValueStarted`]: Event::ValueStarted
//! [`ValueProgressed`]: Event::ValueProgressed
//! [`ValueCompleted`]: Event::ValueCompleted

use crate::validate::Validation;
use crate::value::Value;

/// What kind of value just started, known from its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Object,
    Array,
    String,
    Number,
    Bool,
    Null,
}

/// A change to the document.
///
/// Paths are RFC 6901 JSON Pointers, so they can be handed straight back to
/// [`Stream::status`](crate::Stream::status). The root is `""`.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A value began at `path`.
    ValueStarted { path: String, kind: ValueKind },

    /// An open string grew. `stable_prefix` is the decoded text that can no
    /// longer change — a dangling escape or a half-arrived multi-byte
    /// character contributes nothing until it resolves.
    ///
    /// Emitted at most **once per [`push`](crate::Stream::push)**, not once per
    /// byte: a consumer drains after feeding a chunk, so per-byte events would
    /// be pure overhead and would make the total payload quadratic in the
    /// length of the string.
    ValueProgressed { path: String, stable_prefix: String },

    /// A value finished. The stability guarantee applies: this value will not
    /// change, whatever arrives next — even if the document later turns out to
    /// be malformed.
    ValueCompleted { path: String, value: Value },

    /// A constraint at `path` failed.
    ///
    /// A domain outcome, not an exception: the consumer is *supposed* to keep
    /// receiving events and decide whether to cancel. Only a parser failure --
    /// malformed input, or a number-profile violation -- terminates a stream.
    ///
    /// When `state` is [`Validation::IrrecoverablyInvalid`] no continuation can
    /// repair it, which is the signal to stop generating.
    ///
    /// [`Validation::IrrecoverablyInvalid`]: crate::Validation::IrrecoverablyInvalid
    ValidationFailed { path: String, state: Validation },

    /// A value at `path` finished and its validation settled.
    ValidationCompleted { path: String, state: Validation },

    /// The root value closed. Emitted exactly once, last.
    DocumentCompleted,
}

impl Event {
    /// The path this event concerns; `None` for [`Event::DocumentCompleted`],
    /// which is about the document as a whole.
    pub fn path(&self) -> Option<&str> {
        match self {
            Event::ValueStarted { path, .. }
            | Event::ValueProgressed { path, .. }
            | Event::ValueCompleted { path, .. }
            | Event::ValidationFailed { path, .. }
            | Event::ValidationCompleted { path, .. } => Some(path),
            Event::DocumentCompleted => None,
        }
    }
}

/// Escape one JSON Pointer reference token (RFC 6901 §3): `~` then `/`.
pub(crate) fn escape_token(token: &str) -> String {
    if token.contains('~') || token.contains('/') {
        token.replace('~', "~0").replace('/', "~1")
    } else {
        token.to_string()
    }
}
