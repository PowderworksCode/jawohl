# Test corpora

## JSONTestSuite/

From [nst/JSONTestSuite](https://github.com/nst/JSONTestSuite) by Nicolas
Seriot — the reference conformance suite for JSON parsers, accompanying
["Parsing JSON is a Minefield"](https://seriot.ch/projects/parsing_json.html).
MIT licensed; see `JSONTestSuite/LICENSE`.

File names carry their expectation:

| prefix | meaning |
|---|---|
| `y_` | must be **accepted** as a complete document |
| `n_` | must **not** be accepted as a complete document |
| `i_` | implementation-defined; either answer is conforming |

Two files from the upstream suite are **not** vendored —
`n_structure_100000_opening_arrays.json` (100 KB) and
`n_structure_open_array_object.json` (250 KB). They are mechanically
generated, so `tests/conformance.rs` builds them at run time instead of
carrying 350 KB of repetition in the tree.

### A note on `n_` and streaming

jawohl is a *prefix* parser, so many `n_` cases are not errors for it: `{`,
`["a`, and `{"a":"a` are all perfectly good beginnings of a valid document.
The conformance test therefore asserts the weaker, correct property — that an
`n_` case is never **accepted as a complete document** — rather than requiring
it to fail outright.
