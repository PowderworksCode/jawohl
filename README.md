<p align="center">
  <img src="images/genau-removebg.png" alt="jawohl" width="180">
</p>

# jawohl

**An incremental parser and validator for streaming JSON.** Parse and validate
model output while it is still being generated.

Structured output should behave like a value that materialises progressively,
not a blob of text that becomes useful only when generation finishes. Instead of
waiting for

```json
{"query":"rust parser","limit":10}
```

jawohl can already tell you, from `{"query":"rust par`, that `/query` exists,
that its value so far is `rust par`, and that it is **not yet final**.

```toml
[dependencies]
jawohl = "0.2"
```

## Three levels

**1 — complete a truncated document.** The original entry point, now built on a
real parser:

```rust
use jawohl::complete_json;

assert_eq!(complete_json(r#"{"a":"x"#)?,  r#"{"a":"x"}"#);
assert_eq!(complete_json(r#"{"a": tru"#)?, r#"{"a": true}"#);  // finished
assert_eq!(complete_json(r#"{"a": "x\"#)?, r#"{"a": "x"}"#);   // dangling escape dropped
assert!(complete_json(r#"{"a": 1}}"#).is_err());               // malformed is an error
# Ok::<(), jawohl::ParseError>(())
```

A complete document comes back **byte-identical** — your formatting is not
rewritten.

**2 — stream it.** Push chunks; ask what is final.

```rust
use jawohl::{Stream, Syntax};

let mut s = Stream::new();
s.push(br#"{"query":"rust par"#)?;
assert_eq!(s.status("/query"), Syntax::Incomplete);

s.push(br#"ser","limit":10"#)?;
assert_eq!(s.status("/query"), Syntax::Complete);    // the quote closed it
assert_eq!(s.status("/limit"), Syntax::Incomplete);  // 10 could still become 100
# Ok::<(), jawohl::ParseError>(())
```

`s.changes()` drains an append-only log — `ValueStarted`, `ValueProgressed`,
`ValueCompleted`, `DocumentCompleted` — so you learn *what changed* rather than
diffing snapshots.

**3 — validate against a JSON Schema, as it arrives.**

```rust
use jawohl::{Stream, Validation};

let mut s = Stream::from_json_schema(r#"{"properties":{"role":{"enum":["user","admin"]}}}"#)?;
s.push(br#"{"role":"sup"#)?;

// No member begins "sup", and none ever will.
assert_eq!(s.validation("/role"), Validation::IrrecoverablyInvalid);
assert!(s.is_irrecoverable());   // stop generating
# Ok::<(), Box<dyn std::error::Error>>(())
```

## The stability guarantee

> **Once a path reports `Complete`, no further input can change its value.**

That is what makes it safe to act on a value — fire the search, dispatch the
tool call — while the rest of the document is still arriving. It is enforced
structurally, and it costs something:

- **A number is complete only once a delimiter proves it ended.** `10` may still
  become `100`. This is the rule naive implementations miss, and it is invisible
  in testing because the output parses either way.
- **A string publishes only its decoded-stable prefix.** A dangling `\`, a
  half-written `\u00`, or half a multi-byte character contribute nothing until
  they resolve.
- **A literal completes on its last byte**, needing no delimiter.

Completion is irrevocable, even if the document *later* turns out malformed. Act
on `ValueCompleted` for latency, or wait for `DocumentCompleted` for atomicity —
jawohl exposes the choice rather than making it.

## Early cancellation

Incremental validation pays for itself by letting you stop, not merely by
telling you sooner. From `cargo run --example validate`:

```text
bad role  : CANCELLED after 10 of 44 bytes -- {"role":"s
bad limit : CANCELLED after 27 of 43 bytes -- {"role":"user","limit":1000
extra key : CANCELLED after 23 of 34 bytes -- {"role":"user","nope":1
long query: CANCELLED after 55 of 62 bytes -- {"role":"user","limit":5,"query":"aaaaa…
good      : read all 42 bytes -> Valid
```

Some constraints decide early and some cannot, and jawohl is precise about
which. `enum` and `maxLength` seal shut the moment they are violated; `minLength`
seals open once satisfied; `multipleOf` cannot be judged at all until the value
is whole.

**Numeric bounds are the sharp case.** `1000` against `maximum: 100` looks
obviously doomed — but `1000` may still become `1000e-9`. Under exact analysis no
numeric bound is decidable before the number is delimited, which would delete the
feature. So the assumption is explicit *and enforced*: the default
`NumberProfile::PlainDecimal` assumes no exponent and decides early, and if an
exponent ever appears the stream fails with a named error rather than quietly
pretending the earlier verdict was fine. `NumberProfile::Exact` accepts all JSON
and gives up early numeric rejection.

Anything jawohl could not lower is reported, never silently skipped:

```text
2 constraints compiled
2 unsupported:
  if at /if: conditional application is not lowered yet
  pattern at /pattern: unanchored: checked at completion, but no partial value
    can be rejected early (any prefix can still be extended into a match)
```

That last line is worth reading twice. A `pattern` without a leading `^` is an
*unanchored* search, so no prefix of it is ever irrecoverable — more text can
always be appended that matches. jawohl says so instead of implying a guarantee
it cannot give.

## Examples

```sh
cargo run --example complete    # finish truncated documents
cargo run --example streaming   # watch a tool call materialise, chunk by chunk
cargo run --example validate    # early cancellation, and the lowering report
cargo run --example sse         # the realistic shape: JSON inside a data: stream
```

## Framing

jawohl parses JSON, not the envelope around it. Provider streams wrap fragments
in server-sent events, so the glue is yours — and short:

```rust
# let wire = "data: {\"a\":1}\n";
# let mut stream = jawohl::Stream::new();
for line in wire.lines() {
    if let Some(payload) = line.strip_prefix("data: ") {
        if payload == "[DONE]" { break; }
        stream.push(payload.as_bytes())?;
    }
}
# Ok::<(), jawohl::ParseError>(())
```

See `examples/sse.rs`.

## Upgrading from 0.1

`complete_json` and `get_closing_string_for_partial_json` keep their signatures.
The behaviour is stricter and more correct:

- **Malformed input now returns `Err`.** 0.1 returned `Ok` with output that did
  not parse — for `{"a": tru`, `{"a": "x\`, `{"a":`, `{"a":1,`, `{"que`, and a
  truncated `\u` escape. Six of ten representative prefixes produced invalid
  JSON, all reported as success.
- `MalformedJsonError` is now `ParseError`, carrying a byte offset and a reason.
  The old name remains as a deprecated alias.
- `get_closing_string_for_partial_json` is exact only when the completion needed
  no truncation. A suffix cannot express *dropping* an unfinished fragment, so
  prefer `complete_json`.

## Limits, stated plainly

- **JSON Schema 2020-12**, with local `$ref` (including recursion). Remote `$ref`
  is refused rather than fetched — jawohl does no I/O.
- `anyOf`, `oneOf` and `not` are judged **at completion**, not incrementally.
  Judging them on a prefix needs per-branch state; `Pending` is conservative
  rather than a guess. `allOf` composes incrementally.
- `unevaluatedProperties`, `if`/`then`/`else`, `patternProperties`,
  `dependentSchemas` and `contains` are **not lowered** — and are reported.
- Nesting is capped at 1024 by default (`Stream::with_max_depth`). Input is
  untrusted model output and every level costs an allocation.

## Not yet

Bindings for other languages are the point of this project, and **they do not
exist yet**. They are being built through [jedem][], a sibling that projects a
Rust crate's functions into other languages. Until they ship, this README will
not claim them — the 0.1 README promised JavaScript and Python wrappers "soon"
and never delivered, and once is enough.

## License

MIT.

[jedem]: https://github.com/PowderworksCode/jedem
