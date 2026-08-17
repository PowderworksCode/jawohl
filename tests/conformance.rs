//! Conformance against the reference JSON test suite, fed **one byte at a
//! time**.
//!
//! Two properties are under test here, and the second is the interesting one:
//!
//! 1. **Agreement on complete documents.** A `y_` case is accepted, an `n_`
//!    case is never accepted as complete. Ordinary parser conformance.
//! 2. **The prefix property.** For every valid document and *every* prefix of
//!    it, the parser accepts the prefix without error, and `complete_json`
//!    turns that prefix into something that actually parses. This is what
//!    makes jawohl a streaming parser rather than a batch one, and no amount
//!    of whole-document conformance implies it.
//!
//! Everything is fed byte by byte, so a passing run is also proof that the
//! machine resumes correctly at every possible chunk boundary — including
//! inside escapes and inside multi-byte characters.
//!
//! See `tests/corpus/README.md` for provenance and for why `n_` cases assert
//! "not accepted as complete" rather than "errors".

use jawohl::{complete_json, Stream};
use std::path::{Path, PathBuf};

const CORPUS: &str = "tests/corpus/JSONTestSuite";
const REALWORLD: &str = "tests/corpus/realworld";

fn corpus_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(CORPUS);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

/// The realistic half of the corpus: documents shaped like what jawohl
/// actually sees — tool calls, chat completions, configs — every one of them
/// valid, and all of them larger than the reference suite's specimens.
fn realworld_files() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join(REALWORLD);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    v.sort();
    v
}

fn name(p: &Path) -> String {
    p.file_name().unwrap().to_string_lossy().into_owned()
}

/// Feed `bytes` one at a time. Returns whether the stream survived.
fn feed_by_byte(bytes: &[u8]) -> (Stream, bool) {
    let mut s = Stream::new();
    for b in bytes {
        if s.push(&[*b]).is_err() {
            return (s, false);
        }
    }
    (s, true)
}

/// Accepted as a complete, valid document?
fn accepts(bytes: &[u8]) -> bool {
    let (mut s, ok) = feed_by_byte(bytes);
    ok && s.finish().is_ok() && s.is_document_complete()
}

#[test]
fn y_cases_are_accepted() {
    let mut failures = Vec::new();
    for path in corpus_files() {
        let n = name(&path);
        if !n.starts_with("y_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        if !accepts(&bytes) {
            let (s, _) = feed_by_byte(&bytes);
            failures.push(format!("{n}: {:?}", s.error()));
        }
    }
    assert!(
        failures.is_empty(),
        "must accept but did not:\n{}",
        failures.join("\n")
    );
}

#[test]
fn n_cases_are_never_accepted_as_complete() {
    let mut failures = Vec::new();
    for path in corpus_files() {
        let n = name(&path);
        if !n.starts_with("n_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        if accepts(&bytes) {
            failures.push(n);
        }
    }
    assert!(
        failures.is_empty(),
        "accepted as complete documents but must not be:\n{}",
        failures.join("\n")
    );
}

#[test]
fn i_cases_do_not_panic() {
    // Implementation-defined: either answer conforms. What must not happen is
    // a panic or a hang.
    let mut accepted = 0;
    let mut rejected = 0;
    for path in corpus_files() {
        if !name(&path).starts_with("i_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        if accepts(&bytes) {
            accepted += 1;
        } else {
            rejected += 1;
        }
    }
    assert_eq!(accepted + rejected, 35, "expected the full i_ set");
}

/// The property that matters for streaming: **every prefix of a valid document
/// is itself acceptable, and completable into valid JSON.**
#[test]
fn every_prefix_of_every_valid_document_completes_to_valid_json() {
    let mut failures = Vec::new();
    for path in corpus_files() {
        let n = name(&path);
        if !n.starts_with("y_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue; // complete_json takes &str; non-UTF-8 y_ cases are covered above
        };

        for end in 0..=text.len() {
            if !text.is_char_boundary(end) {
                continue;
            }
            let prefix = &text[..end];

            // 1. a prefix of valid JSON must never be rejected
            let (_, ok) = feed_by_byte(prefix.as_bytes());
            if !ok {
                failures.push(format!("{n}: prefix of {end} bytes rejected: {prefix:?}"));
                continue;
            }

            // 2. and must be completable into something that parses
            match complete_json(prefix) {
                Ok(done) => {
                    if done.trim().is_empty() {
                        continue; // nothing decidable yet, e.g. "" or "  "
                    }
                    if serde_json::from_str::<serde_json::Value>(&done).is_err() {
                        failures.push(format!(
                            "{n}: prefix {prefix:?} completed to {done:?}, which does not parse"
                        ));
                    }
                }
                Err(e) => failures.push(format!("{n}: complete_json({prefix:?}) failed: {e}")),
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} prefix failures:\n{}",
        failures.len(),
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

#[test]
fn valid_documents_round_trip_byte_identical() {
    let mut failures = Vec::new();
    for path in corpus_files() {
        let n = name(&path);
        if !n.starts_with("y_") {
            continue;
        }
        let bytes = std::fs::read(&path).unwrap();
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        match complete_json(text) {
            Ok(out) if out == text => {}
            Ok(out) => failures.push(format!("{n}: {text:?} -> {out:?}")),
            Err(e) => failures.push(format!("{n}: {e}")),
        }
    }
    assert!(
        failures.is_empty(),
        "a complete document must come back untouched:\n{}",
        failures.join("\n")
    );
}

#[test]
fn byte_at_a_time_agrees_with_all_at_once() {
    let mut failures = Vec::new();
    for path in corpus_files() {
        let bytes = std::fs::read(&path).unwrap();
        let (stepwise, step_ok) = feed_by_byte(&bytes);
        let mut bulk = Stream::new();
        let bulk_ok = bulk.push(&bytes).is_ok();
        if step_ok != bulk_ok || stepwise.snapshot() != bulk.snapshot() {
            failures.push(name(&path));
        }
    }
    assert!(
        failures.is_empty(),
        "chunking changed the outcome:\n{}",
        failures.join("\n")
    );
}

// ---- the two pathological cases, generated rather than vendored -------------

#[test]
fn pathological_nesting_is_refused_by_the_depth_limit() {
    // Upstream n_structure_100000_opening_arrays.json. Every `[` is a legal
    // prefix in isolation, so this is not a grammar error — it is refused by
    // the depth limit, which exists precisely for input like this. Before the
    // limit was added, emitting a path per level made this quadratic and the
    // test wedged.
    let deep = "[".repeat(100_000);
    let (s, ok) = feed_by_byte(deep.as_bytes());
    assert!(!ok, "unbounded nesting must be refused");
    assert!(
        matches!(
            s.error().map(|e| &e.kind),
            Some(jawohl::ParseErrorKind::DepthLimitExceeded { .. })
        ),
        "expected a depth-limit error, got {:?}",
        s.error()
    );
    assert!(!accepts(deep.as_bytes()));
}

#[test]
fn pathological_open_objects_are_refused_too() {
    // Upstream n_structure_open_array_object.json: `[{"":` repeated.
    let deep = r#"[{"":"#.repeat(50_000);
    let (_, ok) = feed_by_byte(deep.as_bytes());
    assert!(!ok);
    assert!(!accepts(deep.as_bytes()));
}

#[test]
fn nesting_just_under_the_limit_still_works() {
    let depth = jawohl::DEFAULT_MAX_DEPTH;
    let doc = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(accepts(doc.as_bytes()), "depth {depth} must be accepted");
    assert_eq!(complete_json(&doc).unwrap(), doc);

    let too_deep = format!("{}{}", "[".repeat(depth + 1), "]".repeat(depth + 1));
    assert!(
        !accepts(too_deep.as_bytes()),
        "depth {} must not be",
        depth + 1
    );
}

#[test]
fn the_depth_limit_is_configurable() {
    let doc = "[[[[[1]]]]]";
    let mut shallow = Stream::new().with_max_depth(3);
    assert!(shallow.push(doc.as_bytes()).is_err());

    let mut deep = Stream::new().with_max_depth(10);
    assert!(deep.push(doc.as_bytes()).is_ok());
}

#[test]
fn deep_but_closed_nesting_round_trips() {
    // 500 nested arrays, closed — a real value this deep must survive being
    // built, inspected and dropped.
    let depth = 500;
    let doc = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    assert!(accepts(doc.as_bytes()));
    assert_eq!(complete_json(&doc).unwrap(), doc);
}

/// Guards against the whole suite passing vacuously — a corpus test that
/// silently reads zero files is worse than no test at all. The numbers are
/// asserted exactly so that losing the corpus fails loudly rather than
/// quietly reducing coverage to nothing.
#[test]
fn the_corpus_is_actually_being_exercised() {
    let files = corpus_files();
    let y = files.iter().filter(|p| name(p).starts_with("y_")).count();
    let n = files.iter().filter(|p| name(p).starts_with("n_")).count();
    let i = files.iter().filter(|p| name(p).starts_with("i_")).count();
    assert_eq!((y, n, i), (95, 186, 35), "reference corpus counts changed");
    assert_eq!(
        realworld_files().len(),
        10,
        "realistic corpus counts changed"
    );

    let count_prefixes = |paths: Vec<PathBuf>| -> usize {
        paths
            .iter()
            .filter_map(|p| std::fs::read(p).ok())
            .filter_map(|b| String::from_utf8(b).ok())
            .map(|t| (0..=t.len()).filter(|e| t.is_char_boundary(*e)).count())
            .sum()
    };
    let suite = count_prefixes(
        files
            .into_iter()
            .filter(|p| name(p).starts_with("y_"))
            .collect(),
    );
    let real = count_prefixes(realworld_files());

    // The reference suite's valid cases are deliberately tiny; the realistic
    // corpus is what gives the prefix property real reach.
    assert!(suite > 1_000, "only {suite} reference prefixes");
    assert!(real > 6_000, "only {real} realistic prefixes");
    println!(
        "exercised: {y} y_, {n} n_, {i} i_; {suite} + {real} = {} prefixes",
        suite + real
    );
}

/// The prefix property again, over documents of realistic size and shape.
/// Every prefix of a real tool call must be acceptable and completable.
#[test]
fn every_prefix_of_every_realistic_document_completes_to_valid_json() {
    let mut failures = Vec::new();
    let mut checked = 0usize;
    for path in realworld_files() {
        let n = name(&path);
        let bytes = std::fs::read(&path).unwrap();
        let text = std::str::from_utf8(&bytes).unwrap_or_else(|e| panic!("{n} is not UTF-8: {e}"));

        // The whole document must be accepted and returned untouched.
        assert!(accepts(&bytes), "{n}: not accepted as a complete document");
        assert_eq!(
            complete_json(text).unwrap(),
            text,
            "{n}: not byte-identical"
        );

        for end in 0..=text.len() {
            if !text.is_char_boundary(end) {
                continue;
            }
            let prefix = &text[..end];
            checked += 1;

            let (_, ok) = feed_by_byte(prefix.as_bytes());
            if !ok {
                failures.push(format!("{n}: {end}-byte prefix rejected"));
                continue;
            }
            match complete_json(prefix) {
                Ok(done) => {
                    if done.trim().is_empty() {
                        continue;
                    }
                    if serde_json::from_str::<serde_json::Value>(&done).is_err() {
                        failures.push(format!(
                            "{n}: {end}-byte prefix completed to {done:?}, which does not parse"
                        ));
                    }
                }
                Err(e) => failures.push(format!("{n}: complete_json at {end} failed: {e}")),
            }
        }
    }
    assert!(checked > 6_000, "only {checked} prefixes walked");
    assert!(
        failures.is_empty(),
        "{} failures out of {checked} prefixes:\n{}",
        failures.len(),
        failures
            .iter()
            .take(25)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}

/// Chunk boundaries must not matter, at realistic sizes: every two-way split
/// of every realistic document must agree with feeding it whole.
#[test]
fn every_split_of_every_realistic_document_agrees() {
    for path in realworld_files() {
        let n = name(&path);
        let bytes = std::fs::read(&path).unwrap();
        let mut whole = Stream::new();
        whole.push(&bytes).unwrap();
        let expected = whole.snapshot();

        for split in 0..=bytes.len() {
            let mut s = Stream::new();
            s.push(&bytes[..split]).unwrap();
            s.push(&bytes[split..]).unwrap();
            assert_eq!(
                s.snapshot(),
                expected,
                "{n}: split at byte {split} changed the result"
            );
        }
    }
}
