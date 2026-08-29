# Agent field guide

Durable notes for anyone — human or agent — starting work in this repository.
Append what you learn; keep it to things that are true and not obvious from the
code.

## What this is

jawohl is a single Rust crate: an incremental parser and validator for streaming
JSON, meant for model output that is still being generated. It answers two
questions a batch parser cannot — what does the prefix already prove, and which
of those answers can no longer change.

Everything in the crate hangs off one promise, stated in README as *the
stability guarantee*: once a path reports `Syntax::Complete`, no further input
can change its value. If you are changing the parser, that is the invariant you
are defending, not a nicety.

## Layout

`src/` is the whole library; there is no workspace.

| file | what lives there |
| --- | --- |
| `src/parser.rs` | the push-driven byte-level state machine — the largest file and where the stability guarantee is structurally enforced |
| `src/validate.rs` | what a *prefix* already proves, dispatching on each constraint's monotonicity |
| `src/schema/mod.rs` | JSON Schema lowered into a constraint IR; `Monotonicity` is the organising idea |
| `src/schema/compile.rs` | the lowering itself — note that it parses the schema document with jawohl's own parser, which is why the default build has no JSON dependency |
| `src/schema/trie.rs` | prefix trie over `enum` members; this is why `enum` can be rejected early and exactly |
| `src/schema/pattern.rs` | `pattern` via `regex-automata`; only anchored (`^…`) patterns can ever produce an early verdict |
| `src/event.rs` | the append-only change log and its ordering guarantees |

The design document is `notes/DESIGN.md`. It is written for jawohl 2.0 and
predates the current code, so read it as argument rather than as a description
of what ships; three source files cite it by section number, so if you renumber
it, grep for `notes/DESIGN.md` and fix the citations.

## Building and testing

Nothing beyond a stable toolchain — no sibling checkouts, no system packages, no
`rust-toolchain.toml`. What CI runs (`.github/workflows/ci.yml`):

```sh
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
cargo test --doc
cargo package
for ex in complete streaming validate sse; do cargo run --example "$ex"; done
```

Things worth knowing before you are surprised by a red run:

- **`cargo test --doc` compiles the README.** `src/lib.rs` pulls it in with
  `#[doc = include_str!("../README.md")]`, so every Rust snippet in the front
  page is a doctest. Editing README can break the build, and a snippet that
  needs setup uses `#` -prefixed hidden lines.
- **The examples are a CI job.** They are documentation that has to keep
  working, and their printed output is quoted in README.
- **MSRV is enforced, and the job deletes `Cargo.lock` to do it.** `rust-version`
  is 1.70; the committed lockfile is v4, which cargo 1.70 cannot parse. The job
  resolves fresh instead, which also means it fails when a *dependency* raises
  its MSRV past ours. Dev-dependencies are deliberately outside that promise —
  `serde_json`'s tree already needs 1.71.
- **`cargo package` is gated.** `Cargo.toml` excludes `tests/corpus/**`,
  `tests/conformance.rs` and `images/**` from the published crate, because
  shipping the conformance test without its corpus would ship a test that cannot
  pass. Adding a test that reads from `tests/corpus/` and is *not* excluded will
  break packaging rather than testing.

`fleet-lint.yml` is distributed by conf and should not be edited here; a local
change is drift the next fleet sync reports. Its `hawk` job pins Rust 1.98.0,
runs only when Rust files changed, and is advisory — findings surface as a
warning annotation, never a failure.

## Landmines

**The conformance corpus is a real gate.** `tests/conformance.rs` walks 316
files in `tests/corpus/JSONTestSuite` (vendored from nst/JSONTestSuite) plus 10
in `tests/corpus/realworld`, feeding every one **byte at a time**, and asserts
the prefix property over *every* prefix of every valid document. It is the
slowest thing in the suite and the thing most likely to catch a resumption bug
at a chunk boundary. Two upstream files are not vendored — the 100 KB and 250 KB
mechanically-generated ones — and the test builds them at run time instead.

**`n_` cases do not mean "must error".** jawohl is a prefix parser, so `{` and
`["a` are perfectly good beginnings. The test asserts the weaker, correct
property: an `n_` case is never *accepted as a complete document*. Do not
"strengthen" it.

**The realworld corpus is generated, and the generator writes relative paths.**
`tests/corpus/realworld/generate.py` has `out_dir = "realworld"` hard-coded, so
it must be run from `tests/corpus/`, not from the repository root and not from
its own directory:

```sh
cd tests/corpus && python3 realworld/generate.py
```

Editing a document by hand leaves it inconsistent with the generator; edit
`generate.py` and re-run it. It validates each document with `json.loads` before
writing, so the corpus cannot lie about being valid.

**`Cargo.lock` is committed** even though this is a library. Keep it that way —
the MSRV job's `rm -f Cargo.lock` is deliberate and local to that job.

## Where the reasoning is written down, not the code

Several decisions look arbitrary until you find the paragraph explaining them.
They are all in module-level doc comments:

- Why an undelimited number is never `Complete` (`10` may become `100`) —
  `src/parser.rs`.
- Why unanchored `pattern` can never reject early, and why saying so beats
  implying a guarantee — `src/schema/pattern.rs`.
- Why the event log is a log and not a diff over snapshots — `src/event.rs`.
- Why `anyOf` / `oneOf` / `not` return `Pending` rather than a guess —
  `src/validate.rs`.
- `NumberProfile::PlainDecimal` is an *assumption that is enforced*: it decides
  numeric bounds early on the premise that no exponent appears, and if one does
  the stream fails with a named error rather than pretending the earlier verdict
  held. `NumberProfile::Exact` gives up early numeric rejection instead.

## Naming

The crate version is 0.2.0 and the CHANGELOG's top entry is 0.2.0.
`notes/DESIGN.md` talks about "jawohl 2.0" superseding "1.0" — same rewrite,
different numbering, written before the release settled on 0.2. Trust
`Cargo.toml` and `CHANGELOG.md`.

Language bindings are the point of the project and do not exist yet. They are
meant to come from [jedem](https://github.com/PowderworksCode/jedem), a sibling
that projects a Rust crate's functions into other languages. README declines to
promise them because the 0.1 README promised them and did not deliver; keep that
restraint.
