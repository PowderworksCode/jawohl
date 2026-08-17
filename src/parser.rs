//! The incremental parser: a push-driven, byte-level, resumable state machine.
//!
//! Each byte is consumed exactly once. The machine can be suspended between any
//! two bytes — including in the middle of a `\uXXXX` escape or a multi-byte
//! UTF-8 sequence — because chunk boundaries in a token stream are arbitrary.
//!
//! The stability guarantee ("once Complete, the value cannot change") is
//! enforced structurally here, by three rules:
//!
//! * a **number** completes only when a delimiter proves it ended — `10` may
//!   still become `100`, so an undelimited number is never Complete;
//! * a **string** publishes only its decoded-stable prefix — a dangling `\` or
//!   a partial `\u00` contributes nothing until it resolves, and a split
//!   multi-byte character is withheld until its last byte arrives;
//! * a **literal** completes on its final byte, since no longer JSON token has
//!   `true`, `false` or `null` as a prefix.

use crate::error::{ParseError, ParseErrorKind};
use crate::value::{Number, Value};

/// What the machine is waiting for at a container boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Expect {
    /// A value: at document start, after `[`, after `,` in an array, after `:`.
    Value,
    /// `}` or a key, immediately after `{`.
    KeyOrEnd,
    /// A key, after `,` in an object.
    Key,
    /// `:` between a key and its value.
    Colon,
    /// `,` or the container's closing bracket.
    CommaOrEnd,
    /// Nothing more; the root value is finished.
    Done,
}

#[derive(Debug)]
enum Frame {
    Object {
        members: Vec<(String, Value)>,
        /// A key that has been read but whose value has not arrived.
        pending_key: Option<String>,
    },
    Array {
        items: Vec<Value>,
    },
}

/// Where inside a string literal we are.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StrState {
    Raw,
    Backslash,
    /// `\u`, with the hex digits gathered so far.
    Unicode(u8, u32),
    /// A high surrogate awaiting `\uXXXX` for its low half.
    SurrogateBackslash(u16),
    SurrogateU(u16),
    SurrogateHex(u16, u8, u32),
}

/// Sub-state for JSON's number grammar. Terminable states are those in which
/// the number, if it ended right now, would be well-formed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NumState {
    AfterSign,
    Zero,
    Int,
    Dot,
    Frac,
    Exp,
    ExpSign,
    ExpDigits,
}

impl NumState {
    fn terminable(self) -> bool {
        matches!(
            self,
            NumState::Zero | NumState::Int | NumState::Frac | NumState::ExpDigits
        )
    }
}

#[derive(Debug)]
enum Tok {
    None,
    Str {
        /// Decoded bytes so far (escapes resolved).
        decoded: Vec<u8>,
        state: StrState,
        /// True when this string is an object key rather than a value.
        is_key: bool,
        /// Trailing input bytes belonging to an escape that has not resolved
        /// (`\\`, `\\u00`, a lone high surrogate). They contribute nothing to
        /// the decoded value and must be dropped when completing a document.
        pending: usize,
    },
    Num {
        text: String,
        state: NumState,
    },
    Lit {
        text: String,
    },
}

pub(crate) struct Parser {
    stack: Vec<Frame>,
    expect: Expect,
    tok: Tok,
    root: Option<Value>,
    failed: Option<ParseError>,
    offset: usize,
    /// The largest prefix length at which the input was *structurally
    /// closable* — everything before it is a complete value, so appending the
    /// open containers' closing brackets yields a valid document. Anything
    /// after it is an unfinished fragment.
    commit: usize,
}

impl Parser {
    pub(crate) fn new() -> Self {
        Parser {
            stack: Vec::new(),
            expect: Expect::Value,
            tok: Tok::None,
            root: None,
            failed: None,
            offset: 0,
            commit: 0,
        }
    }

    pub(crate) fn failure(&self) -> Option<&ParseError> {
        self.failed.as_ref()
    }

    /// True once the root value has closed.
    pub(crate) fn document_complete(&self) -> bool {
        self.expect == Expect::Done && matches!(self.tok, Tok::None)
    }

    fn err<T>(&mut self, kind: ParseErrorKind) -> Result<T, ParseError> {
        let e = ParseError {
            offset: self.offset,
            kind,
        };
        if self.failed.is_none() {
            self.failed = Some(e.clone());
        }
        Err(e)
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        for &b in bytes {
            self.step(b)?;
            self.offset += 1;
            if matches!(self.tok, Tok::None) && self.closable() {
                self.commit = self.offset;
            }
        }
        Ok(())
    }

    /// End of input. Completes a trailing number or literal if it is
    /// well-formed; containers left open simply stay open.
    pub(crate) fn finish(&mut self) -> Result<(), ParseError> {
        if let Some(e) = &self.failed {
            return Err(e.clone());
        }
        match &self.tok {
            Tok::Num { state, .. } => {
                if !state.terminable() {
                    return self.err(ParseErrorKind::MalformedNumber);
                }
                self.close_number();
            }
            Tok::Lit { text } => {
                let t = text.clone();
                if !["true", "false", "null"].contains(&t.as_str()) {
                    return self.err(ParseErrorKind::BadLiteral);
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn step(&mut self, b: u8) -> Result<(), ParseError> {
        // A token in progress consumes bytes until it decides otherwise.
        match &mut self.tok {
            Tok::Str { .. } => return self.step_string(b),
            Tok::Num { .. } => {
                if self.step_number(b)? {
                    return Ok(());
                }
                // The byte delimited the number; fall through and re-dispatch it.
            }
            Tok::Lit { .. } => return self.step_literal(b),
            Tok::None => {}
        }
        self.step_structural(b)
    }

    fn step_structural(&mut self, b: u8) -> Result<(), ParseError> {
        if matches!(b, b' ' | b'\t' | b'\n' | b'\r') {
            return Ok(());
        }
        match self.expect {
            Expect::Value => self.begin_value(b),
            Expect::KeyOrEnd => match b {
                b'"' => {
                    self.tok = Tok::Str {
                        decoded: Vec::new(),
                        state: StrState::Raw,
                        is_key: true,
                        pending: 0,
                    };
                    Ok(())
                }
                b'}' => self.close_container(b'}'),
                _ => self.err(ParseErrorKind::Unexpected {
                    byte: b,
                    expected: "a key or `}`",
                }),
            },
            Expect::Key => match b {
                b'"' => {
                    self.tok = Tok::Str {
                        decoded: Vec::new(),
                        state: StrState::Raw,
                        is_key: true,
                        pending: 0,
                    };
                    Ok(())
                }
                _ => self.err(ParseErrorKind::Unexpected {
                    byte: b,
                    expected: "a key",
                }),
            },
            Expect::Colon => match b {
                b':' => {
                    self.expect = Expect::Value;
                    Ok(())
                }
                _ => self.err(ParseErrorKind::Unexpected {
                    byte: b,
                    expected: "`:`",
                }),
            },
            Expect::CommaOrEnd => match b {
                b',' => {
                    self.expect = match self.stack.last() {
                        Some(Frame::Object { .. }) => Expect::Key,
                        _ => Expect::Value,
                    };
                    Ok(())
                }
                b'}' | b']' => self.close_container(b),
                _ => self.err(ParseErrorKind::Unexpected {
                    byte: b,
                    expected: "`,` or a closing bracket",
                }),
            },
            Expect::Done => self.err(ParseErrorKind::TrailingContent { byte: b }),
        }
    }

    fn begin_value(&mut self, b: u8) -> Result<(), ParseError> {
        match b {
            b'{' => {
                self.stack.push(Frame::Object {
                    members: Vec::new(),
                    pending_key: None,
                });
                self.expect = Expect::KeyOrEnd;
                Ok(())
            }
            b'[' => {
                self.stack.push(Frame::Array { items: Vec::new() });
                self.expect = Expect::Value;
                Ok(())
            }
            b']' => {
                // An empty array: `[` then `]`, which lands here as Expect::Value.
                if matches!(self.stack.last(), Some(Frame::Array { items })  if items.is_empty()) {
                    self.close_container(b']')
                } else {
                    self.err(ParseErrorKind::Unexpected {
                        byte: b,
                        expected: "a value",
                    })
                }
            }
            b'"' => {
                self.tok = Tok::Str {
                    decoded: Vec::new(),
                    state: StrState::Raw,
                    is_key: false,
                    pending: 0,
                };
                Ok(())
            }
            b'-' => {
                self.tok = Tok::Num {
                    text: "-".into(),
                    state: NumState::AfterSign,
                };
                Ok(())
            }
            b'0' => {
                self.tok = Tok::Num {
                    text: "0".into(),
                    state: NumState::Zero,
                };
                Ok(())
            }
            b'1'..=b'9' => {
                self.tok = Tok::Num {
                    text: (b as char).to_string(),
                    state: NumState::Int,
                };
                Ok(())
            }
            b't' | b'f' | b'n' => {
                self.tok = Tok::Lit {
                    text: (b as char).to_string(),
                };
                Ok(())
            }
            _ => self.err(ParseErrorKind::Unexpected {
                byte: b,
                expected: "a value",
            }),
        }
    }

    // ---- strings -----------------------------------------------------------

    fn step_string(&mut self, b: u8) -> Result<(), ParseError> {
        let (decoded, state, is_key, pending) = match &mut self.tok {
            Tok::Str {
                decoded,
                state,
                is_key,
                pending,
            } => (decoded, state, *is_key, pending),
            _ => unreachable!(),
        };
        match *state {
            StrState::Raw => match b {
                b'"' => {
                    let bytes = std::mem::take(decoded);
                    let s = match String::from_utf8(bytes) {
                        Ok(s) => s,
                        Err(_) => return self.err(ParseErrorKind::InvalidUtf8),
                    };
                    self.tok = Tok::None;
                    if is_key {
                        if let Some(Frame::Object { pending_key, .. }) = self.stack.last_mut() {
                            *pending_key = Some(s);
                        }
                        self.expect = Expect::Colon;
                    } else {
                        self.attach(Value::String(s));
                    }
                    Ok(())
                }
                b'\\' => {
                    *state = StrState::Backslash;
                    *pending = 1;
                    Ok(())
                }
                0x00..=0x1f => self.err(ParseErrorKind::ControlInString { byte: b }),
                _ => {
                    decoded.push(b);
                    Ok(())
                }
            },
            StrState::Backslash => {
                let repl = match b {
                    b'"' => Some(b'"'),
                    b'\\' => Some(b'\\'),
                    b'/' => Some(b'/'),
                    b'b' => Some(0x08),
                    b'f' => Some(0x0c),
                    b'n' => Some(b'\n'),
                    b'r' => Some(b'\r'),
                    b't' => Some(b'\t'),
                    b'u' => {
                        *state = StrState::Unicode(0, 0);
                        *pending = 2;
                        return Ok(());
                    }
                    _ => None,
                };
                match repl {
                    Some(c) => {
                        decoded.push(c);
                        *state = StrState::Raw;
                        *pending = 0;
                        Ok(())
                    }
                    None => self.err(ParseErrorKind::BadEscape { byte: b }),
                }
            }
            StrState::Unicode(n, acc) => {
                let d = match hex(b) {
                    Some(d) => d,
                    None => return self.err(ParseErrorKind::BadUnicodeEscape { byte: b }),
                };
                let acc = acc * 16 + d;
                if n + 1 < 4 {
                    *state = StrState::Unicode(n + 1, acc);
                    *pending += 1;
                    return Ok(());
                }
                let cp = acc as u16;
                if (0xD800..0xDC00).contains(&cp) {
                    // High surrogate: a low surrogate must follow.
                    *state = StrState::SurrogateBackslash(cp);
                    *pending += 1;
                    return Ok(());
                }
                if (0xDC00..0xE000).contains(&cp) {
                    return self.err(ParseErrorKind::BadSurrogate);
                }
                push_char(decoded, char::from_u32(cp as u32).unwrap());
                *state = StrState::Raw;
                *pending = 0;
                Ok(())
            }
            StrState::SurrogateBackslash(hi) => {
                if b == b'\\' {
                    *state = StrState::SurrogateU(hi);
                    *pending += 1;
                    Ok(())
                } else {
                    self.err(ParseErrorKind::BadSurrogate)
                }
            }
            StrState::SurrogateU(hi) => {
                if b == b'u' {
                    *state = StrState::SurrogateHex(hi, 0, 0);
                    *pending += 1;
                    Ok(())
                } else {
                    self.err(ParseErrorKind::BadSurrogate)
                }
            }
            StrState::SurrogateHex(hi, n, acc) => {
                let d = match hex(b) {
                    Some(d) => d,
                    None => return self.err(ParseErrorKind::BadUnicodeEscape { byte: b }),
                };
                let acc = acc * 16 + d;
                if n + 1 < 4 {
                    *state = StrState::SurrogateHex(hi, n + 1, acc);
                    *pending += 1;
                    return Ok(());
                }
                let lo = acc as u16;
                if !(0xDC00..0xE000).contains(&lo) {
                    return self.err(ParseErrorKind::BadSurrogate);
                }
                let cp = 0x10000 + ((hi as u32 - 0xD800) << 10) + (lo as u32 - 0xDC00);
                match char::from_u32(cp) {
                    Some(c) => {
                        push_char(decoded, c);
                        *state = StrState::Raw;
                        *pending = 0;
                        Ok(())
                    }
                    None => self.err(ParseErrorKind::BadSurrogate),
                }
            }
        }
    }

    // ---- numbers -----------------------------------------------------------

    /// Returns `true` if the byte was consumed by the number, `false` if it
    /// delimited it (in which case the number is closed and the caller
    /// re-dispatches the byte structurally).
    fn step_number(&mut self, b: u8) -> Result<bool, ParseError> {
        let (text, state) = match &mut self.tok {
            Tok::Num { text, state } => (text, state),
            _ => unreachable!(),
        };
        let next = match (*state, b) {
            (NumState::AfterSign, b'0') => Some(NumState::Zero),
            (NumState::AfterSign, b'1'..=b'9') => Some(NumState::Int),
            (NumState::Zero, b'.') => Some(NumState::Dot),
            (NumState::Zero, b'e' | b'E') => Some(NumState::Exp),
            (NumState::Int, b'0'..=b'9') => Some(NumState::Int),
            (NumState::Int, b'.') => Some(NumState::Dot),
            (NumState::Int, b'e' | b'E') => Some(NumState::Exp),
            (NumState::Dot, b'0'..=b'9') => Some(NumState::Frac),
            (NumState::Frac, b'0'..=b'9') => Some(NumState::Frac),
            (NumState::Frac, b'e' | b'E') => Some(NumState::Exp),
            (NumState::Exp, b'+' | b'-') => Some(NumState::ExpSign),
            (NumState::Exp, b'0'..=b'9') => Some(NumState::ExpDigits),
            (NumState::ExpSign, b'0'..=b'9') => Some(NumState::ExpDigits),
            (NumState::ExpDigits, b'0'..=b'9') => Some(NumState::ExpDigits),
            _ => None,
        };
        if let Some(n) = next {
            text.push(b as char);
            *state = n;
            return Ok(true);
        }
        // Not part of the number. Only a genuine delimiter may end it.
        let terminable = state.terminable();
        let delimits = matches!(b, b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r');
        if !delimits || !terminable {
            return self.err(ParseErrorKind::MalformedNumber);
        }
        self.close_number();
        Ok(false)
    }

    fn close_number(&mut self) {
        if let Tok::Num { text, .. } = std::mem::replace(&mut self.tok, Tok::None) {
            self.attach(Value::Number(Number::new(text)));
            // The delimiter is not part of the number, so the safe prefix ends
            // at the delimiter, not after it.
            self.commit = self.offset;
        }
    }

    // ---- literals ----------------------------------------------------------

    fn step_literal(&mut self, b: u8) -> Result<(), ParseError> {
        let text = match &mut self.tok {
            Tok::Lit { text } => text,
            _ => unreachable!(),
        };
        let mut candidate = text.clone();
        candidate.push(b as char);
        let matches_prefix = ["true", "false", "null"]
            .iter()
            .any(|k| k.starts_with(&candidate));
        if matches_prefix {
            text.push(b as char);
            let done = ["true", "false", "null"].contains(&text.as_str());
            if done {
                let v = match text.as_str() {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    _ => Value::Null,
                };
                self.tok = Tok::None;
                self.attach(v);
            }
            return Ok(());
        }
        // The literal is finished only if it already spells a keyword; anything
        // else (`tru}`) is malformed — the failure 1.0 cannot detect.
        self.err(ParseErrorKind::BadLiteral)
    }

    // ---- structure ---------------------------------------------------------

    /// Attach a finished value to its parent (or make it the root) and move to
    /// the state that follows a completed value.
    fn attach(&mut self, v: Value) {
        match self.stack.last_mut() {
            Some(Frame::Object {
                members,
                pending_key,
            }) => {
                let k = pending_key.take().unwrap_or_default();
                members.push((k, v));
                self.expect = Expect::CommaOrEnd;
            }
            Some(Frame::Array { items }) => {
                items.push(v);
                self.expect = Expect::CommaOrEnd;
            }
            None => {
                self.root = Some(v);
                self.expect = Expect::Done;
            }
        }
    }

    fn close_container(&mut self, b: u8) -> Result<(), ParseError> {
        let frame = match self.stack.pop() {
            Some(f) => f,
            None => return self.err(ParseErrorKind::UnbalancedClose { found: b }),
        };
        let (value, open) = match frame {
            Frame::Object { members, .. } => (Value::Object(members), b'{'),
            Frame::Array { items } => (Value::Array(items), b'['),
        };
        let want = if open == b'{' { b'}' } else { b']' };
        if b != want {
            // Put it back so the error offset still describes a real state.
            return self.err(ParseErrorKind::MismatchedClose { found: b, open });
        }
        self.attach(value);
        Ok(())
    }

    /// Is the structure, right now, closable by appending brackets alone?
    fn closable(&self) -> bool {
        match self.expect {
            Expect::Done | Expect::CommaOrEnd | Expect::KeyOrEnd => true,
            // Just after `[`: an empty array is a complete value.
            Expect::Value => {
                matches!(self.stack.last(), Some(Frame::Array { items }) if items.is_empty())
            }
            _ => false,
        }
    }

    /// How to turn the bytes seen so far into a valid document: keep
    /// `keep` bytes of the original input, append `tail`, then append
    /// `closers` (innermost container first).
    ///
    /// Keeping the input verbatim rather than re-serialising matters: a caller
    /// completing a half-streamed document wants their own formatting back,
    /// not a minified rewrite.
    pub(crate) fn completion(&self) -> Completion {
        let mut closers = String::new();
        for frame in self.stack.iter().rev() {
            closers.push(match frame {
                Frame::Object { .. } => '}',
                Frame::Array { .. } => ']',
            });
        }
        let (keep, tail) = match &self.tok {
            // A value string can always be closed after its stable prefix;
            // any unresolved escape bytes are dropped.
            Tok::Str {
                is_key: false,
                pending,
                ..
            } => (self.offset - pending, "\"".to_string()),
            // A literal is unambiguously completable: only one of true/false/
            // null can have this prefix.
            Tok::Lit { text } => {
                let full = ["true", "false", "null"]
                    .iter()
                    .find(|k| k.starts_with(text.as_str()))
                    .copied()
                    .unwrap_or("");
                (self.offset, full[text.len()..].to_string())
            }
            // A number that is already well-formed stands; one that is not
            // (`1e`, `-`) is dropped along with its member.
            Tok::Num { state, .. } if state.terminable() => (self.offset, String::new()),
            // A partial key, a half-written number, a key with no value, a
            // trailing comma: rewind to the last closable point.
            _ => (self.commit, String::new()),
        };
        Completion {
            keep,
            tail,
            closers,
        }
    }

    // ---- inspection --------------------------------------------------------

    /// The document as it currently stands, including any in-flight token.
    pub(crate) fn snapshot(&self) -> Option<Value> {
        let mut pending = self.token_value();
        // Rebuild from the innermost frame outwards.
        for frame in self.stack.iter().rev() {
            let v = match frame {
                Frame::Object {
                    members,
                    pending_key,
                } => {
                    let mut m = members.clone();
                    if let Some(p) = pending.take() {
                        m.push((pending_key.clone().unwrap_or_default(), p));
                    }
                    Value::Object(m)
                }
                Frame::Array { items } => {
                    let mut it = items.clone();
                    if let Some(p) = pending.take() {
                        it.push(p);
                    }
                    Value::Array(it)
                }
            };
            pending = Some(v);
        }
        pending.or_else(|| self.root.clone())
    }

    /// The in-flight token as a partial value, if there is one. A key being
    /// typed is not a value, so it contributes nothing.
    fn token_value(&self) -> Option<Value> {
        match &self.tok {
            Tok::None => None,
            Tok::Str { is_key: true, .. } => None,
            Tok::Str { decoded, .. } => Some(Value::PartialString(stable_prefix(decoded))),
            Tok::Num { text, .. } => Some(Value::PartialNumber(text.clone())),
            Tok::Lit { text } => Some(Value::PartialLiteral(text.clone())),
        }
    }

    /// The path spelled out by the still-open container frames. Every proper
    /// prefix of it (including the empty root path) names a container that has
    /// not yet seen its closing bracket, and so is not `Complete`.
    pub(crate) fn open_frame_path(&self) -> Option<Vec<String>> {
        if self.stack.is_empty() {
            return None;
        }
        let mut path: Vec<String> = Vec::new();
        for frame in &self.stack {
            match frame {
                Frame::Object { pending_key, .. } => {
                    path.push(pending_key.clone().unwrap_or_default())
                }
                Frame::Array { items } => path.push(items.len().to_string()),
            }
        }
        // The last segment addresses the slot *inside* the innermost frame,
        // which is not itself an open container; drop it.
        path.pop();
        Some(path)
    }
}

fn hex(b: u8) -> Option<u32> {
    match b {
        b'0'..=b'9' => Some((b - b'0') as u32),
        b'a'..=b'f' => Some((b - b'a' + 10) as u32),
        b'A'..=b'F' => Some((b - b'A' + 10) as u32),
        _ => None,
    }
}

fn push_char(buf: &mut Vec<u8>, c: char) {
    let mut tmp = [0u8; 4];
    buf.extend_from_slice(c.encode_utf8(&mut tmp).as_bytes());
}

/// The longest prefix of `decoded` that is valid UTF-8. A multi-byte character
/// split across chunk boundaries is withheld until its final byte arrives —
/// publishing half of it would break the stability guarantee.
fn stable_prefix(decoded: &[u8]) -> String {
    match std::str::from_utf8(decoded) {
        Ok(s) => s.to_string(),
        Err(e) => {
            let good = e.valid_up_to();
            // Safe: valid_up_to() is by definition a UTF-8 boundary.
            String::from_utf8_lossy(&decoded[..good]).into_owned()
        }
    }
}

/// The longest prefix of a partially-written number that is itself a complete,
/// well-formed JSON number. `1e` yields `1`, `1.` yields `1`, `-` yields none.
/// Used when completing a truncated document: emit the most that is certainly
/// valid, drop what is not.
pub(crate) fn terminable_number_prefix(text: &str) -> Option<&str> {
    let mut state: Option<NumState> = None;
    let mut best = 0usize;
    for (i, c) in text.char_indices() {
        let b = c as u8;
        let next = match (state, b) {
            (None, b'-') => Some(NumState::AfterSign),
            (None, b'0') => Some(NumState::Zero),
            (None, b'1'..=b'9') => Some(NumState::Int),
            (Some(NumState::AfterSign), b'0') => Some(NumState::Zero),
            (Some(NumState::AfterSign), b'1'..=b'9') => Some(NumState::Int),
            (Some(NumState::Zero), b'.') => Some(NumState::Dot),
            (Some(NumState::Zero), b'e' | b'E') => Some(NumState::Exp),
            (Some(NumState::Int), b'0'..=b'9') => Some(NumState::Int),
            (Some(NumState::Int), b'.') => Some(NumState::Dot),
            (Some(NumState::Int), b'e' | b'E') => Some(NumState::Exp),
            (Some(NumState::Dot), b'0'..=b'9') => Some(NumState::Frac),
            (Some(NumState::Frac), b'0'..=b'9') => Some(NumState::Frac),
            (Some(NumState::Frac), b'e' | b'E') => Some(NumState::Exp),
            (Some(NumState::Exp), b'+' | b'-') => Some(NumState::ExpSign),
            (Some(NumState::Exp), b'0'..=b'9') => Some(NumState::ExpDigits),
            (Some(NumState::ExpSign), b'0'..=b'9') => Some(NumState::ExpDigits),
            (Some(NumState::ExpDigits), b'0'..=b'9') => Some(NumState::ExpDigits),
            _ => return if best > 0 { Some(&text[..best]) } else { None },
        };
        state = next;
        if next.map(|s| s.terminable()).unwrap_or(false) {
            best = i + c.len_utf8();
        }
    }
    if best > 0 {
        Some(&text[..best])
    } else {
        None
    }
}

/// A plan for turning a truncated document into a valid one.
pub(crate) struct Completion {
    /// How many bytes of the original input to keep.
    pub keep: usize,
    /// Text that finishes the value in flight (a closing quote, the rest of a
    /// literal).
    pub tail: String,
    /// Closing brackets for every container still open, innermost first.
    pub closers: String,
}
