# Changelog

## 0.2.0

jawohl stops being a JSON-repair function and becomes **an incremental parser
and validator for streaming JSON**.

### The case for the rewrite

0.1 counted unclosed brackets and appended closers. Ten representative partial
documents through `complete_json`, checking whether the result parses:
**six emit invalid JSON, and all six return `Ok`**.

| input | 0.1 | 0.2 |
|---|---|---|
| `{"a": tru` | `{"a": tru}` ✗ | `{"a": true}` |
| `{"a": "x\` | `{"a": "x\"}` ✗ | `{"a": "x"}` |
| `{"a": "x\u00` | `{"a": "x\u00"}` ✗ | `{"a": "x"}` |
| `{"a":` | `{"a":}` ✗ | `{}` |
| `{"a":1,` | `{"a":1,}` ✗ | `{"a":1}` |
| `{"que` | `{"que"}` ✗ | `{}` |

A closer-counter has no model of where it is in the grammar, so it cannot know
that `tru` needs an `e` or that a trailing `\` escapes the quote it just
appended. That is structural, not a bug list.

### Added

- **`Stream`** — push chunks, inspect state by JSON Pointer. `status()` reports
  `Missing` / `Incomplete` / `Complete`; `snapshot()` gives the partial value.
- **The stability guarantee** — once a path is `Complete`, no further input can
  change its value. Enforced structurally: a number completes only when
  delimited, a string publishes only its decoded-stable prefix, a literal
  completes on its last byte.
- **An event log** — `changes()` drains `ValueStarted`, `ValueProgressed`,
  `ValueCompleted`, `DocumentCompleted`, `ValidationFailed`,
  `ValidationCompleted`, with ordering guarantees.
- **Incremental validation against JSON Schema 2020-12** —
  `Stream::from_json_schema`, `validation(pointer)`, five states, and
  `is_irrecoverable()` for early cancellation.
- **`NumberProfile`** — `PlainDecimal` (default) decides numeric bounds on a
  prefix and fails loudly if an exponent appears; `Exact` is sound for all JSON
  and gives up early numeric rejection.
- **`lowering_report()`** — every schema keyword jawohl could not lower, with
  its location and reason. Nothing is silently skipped.
- **`parse_complete`** — parse a whole document, for the non-streaming case.
- **A depth limit**, 1024 by default, configurable. Input is untrusted model
  output and every nesting level costs an allocation.
- Four runnable examples, and a conformance suite over the reference JSON test
  suite plus a realistic corpus — 8,151 prefixes, fed one byte at a time.

### Changed

- **Malformed input returns `Err`.** It used to return `Ok` with invalid output.
- `complete_json` truncates back to the last structurally closable point and
  appends, rather than re-serialising, so a complete document comes back
  **byte-identical** and a partial one keeps your formatting.
- `MalformedJsonError` → `ParseError`, carrying a byte offset and a reason. The
  old name remains as a deprecated alias.
- Metadata now points at `PowderworksCode/jawohl` and `docs.rs`.

### Deprecated

- `get_closing_string_for_partial_json` is exact only when the completion needed
  no truncation — a suffix cannot express *dropping* an unfinished fragment.
  Prefer `complete_json`.

### Removed

- `bench.rs`, which referenced a crate that does not exist and had not compiled
  since before the project was renamed.
- The 2023 OpenAI example, which pinned three-year-old dependencies, needed an
  API key, and demonstrated only the 0.1 surface. Replaced by four self-contained
  examples that CI runs.

### Known limits

- `anyOf` / `oneOf` / `not` are judged at completion, not incrementally.
- `unevaluatedProperties`, `if`/`then`/`else`, `patternProperties`,
  `dependentSchemas` and `contains` are not lowered — and are reported.
- Remote `$ref` is refused rather than fetched; jawohl does no I/O.
- No bindings for other languages yet. They are the point of the project and are
  being built through [jedem](https://github.com/PowderworksCode/jedem); this
  changelog will claim them when they exist.

## 0.1.1 and earlier

Bracket-counting JSON completion. See the git history.
