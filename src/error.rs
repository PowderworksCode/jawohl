//! Parse failures.
//!
//! A `ParseError` means the input is not a prefix of any valid JSON document.
//! It is deliberately distinct from "incomplete": incompleteness is a normal,
//! expected state during streaming (see [`crate::Syntax`]), while a
//! `ParseError` is terminal — once a stream has failed it stays failed.

use std::error::Error;
use std::fmt;

/// What went wrong, and where.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    /// Byte offset in the overall stream at which the failure was detected.
    pub offset: usize,
    /// The specific failure.
    pub kind: ParseErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ParseErrorKind {
    /// A byte that cannot appear here, with a short note on what was expected.
    Unexpected { byte: u8, expected: &'static str },
    /// `]` closing an object, or `}` closing an array.
    MismatchedClose { found: u8, open: u8 },
    /// A closing bracket with nothing open.
    UnbalancedClose { found: u8 },
    /// Non-whitespace after the top-level value finished.
    TrailingContent { byte: u8 },
    /// `\q` — not a recognised escape.
    BadEscape { byte: u8 },
    /// A `\uXXXX` escape with a non-hex digit.
    BadUnicodeEscape { byte: u8 },
    /// A `\uXXXX` surrogate that cannot form a scalar value.
    BadSurrogate,
    /// A string contained bytes that are not valid UTF-8.
    InvalidUtf8,
    /// A raw control byte (< 0x20) inside a string; JSON requires escaping.
    ControlInString { byte: u8 },
    /// A number that does not match JSON's grammar (`01`, `1.`, `1e`, `-`).
    MalformedNumber,
    /// `tru}` — a literal that started but did not finish.
    BadLiteral,
    /// Containers nested deeper than the configured limit.
    DepthLimitExceeded { limit: usize },
    /// The input ended before the document did, where a complete document was
    /// required.
    UnexpectedEndOfInput,
    /// An exponent appeared while validating under
    /// [`NumberProfile::PlainDecimal`].
    ///
    /// The profile is an assumption that makes numeric bounds decidable on a
    /// prefix. When it turns out false, jawohl says so instead of quietly
    /// re-widening and letting an earlier verdict stand unexamined -- either
    /// the guarantee held, or the caller is told it did not.
    ///
    /// [`NumberProfile::PlainDecimal`]: crate::NumberProfile::PlainDecimal
    NumberProfileViolated,
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        use ParseErrorKind::*;
        write!(f, "at byte {}: ", self.offset)?;
        match &self.kind {
            Unexpected { byte, expected } => {
                write!(f, "unexpected {}, expected {}", ch(*byte), expected)
            }
            MismatchedClose { found, open } => {
                write!(f, "{} closes a value opened with {}", ch(*found), ch(*open))
            }
            UnbalancedClose { found } => write!(f, "{} with nothing open", ch(*found)),
            TrailingContent { byte } => {
                write!(f, "trailing {} after the document ended", ch(*byte))
            }
            BadEscape { byte } => write!(f, "invalid escape \\{}", ch(*byte)),
            BadUnicodeEscape { byte } => {
                write!(f, "invalid hex digit {} in \\u escape", ch(*byte))
            }
            BadSurrogate => write!(f, "invalid surrogate pair in \\u escape"),
            InvalidUtf8 => write!(f, "string is not valid UTF-8"),
            ControlInString { byte } => {
                write!(f, "unescaped control byte {:#04x} in string", byte)
            }
            MalformedNumber => write!(f, "malformed number"),
            BadLiteral => write!(f, "malformed literal (expected true, false or null)"),
            DepthLimitExceeded { limit } => {
                write!(f, "nesting deeper than the limit of {limit}")
            }
            UnexpectedEndOfInput => write!(f, "input ended before the document was complete"),
            NumberProfileViolated => write!(
                f,
                "exponent notation under NumberProfile::PlainDecimal; \
                 earlier numeric verdicts assumed it would not appear. \
                 Use NumberProfile::Exact to accept exponents (at the cost of \
                 no early numeric rejection)"
            ),
        }
    }
}

impl Error for ParseError {}

fn ch(b: u8) -> String {
    if b.is_ascii_graphic() {
        format!("`{}`", b as char)
    } else {
        format!("byte {:#04x}", b)
    }
}

/// Kept for 1.0 source compatibility: `complete_json` used to return this.
/// It is now an alias for the richer [`ParseError`].
#[deprecated(since = "0.2.0", note = "renamed to ParseError")]
#[allow(dead_code)]
pub type MalformedJsonError = ParseError;
