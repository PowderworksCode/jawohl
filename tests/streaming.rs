//! The stability guarantee, and resumability across arbitrary chunk splits.
//!
//! These are the properties that make jawohl more than a JSON parser: a value
//! reported `Complete` will never change, and where the chunk boundaries fall
//! makes no difference to the result.

use jawohl::{Stream, Syntax, Value};

fn feed(chunks: &[&str]) -> Stream {
    let mut s = Stream::new();
    for c in chunks {
        s.push(c.as_bytes()).unwrap();
    }
    s
}

#[test]
fn a_string_completes_at_its_closing_quote() {
    let s = feed(&[r#"{"query":"rust par"#]);
    assert_eq!(s.status("/query"), Syntax::Incomplete);

    let s = feed(&[r#"{"query":"rust parser""#]);
    assert_eq!(s.status("/query"), Syntax::Complete);
}

#[test]
fn a_number_is_incomplete_until_delimited() {
    // The single most common way to break the guarantee: 10 may become 100.
    let s = feed(&[r#"{"limit":10"#]);
    assert_eq!(s.status("/limit"), Syntax::Incomplete);

    let s = feed(&[r#"{"limit":10,"#]);
    assert_eq!(s.status("/limit"), Syntax::Complete);

    let s = feed(&[r#"{"limit":10}"#]);
    assert_eq!(s.status("/limit"), Syntax::Complete);

    let s = feed(&[r#"{"limit":10 "#]);
    assert_eq!(s.status("/limit"), Syntax::Complete);
}

#[test]
fn the_number_that_grew() {
    // Feed a digit at a time; the value must not be reported Complete at any
    // point where a further digit could still change it.
    let mut s = Stream::new();
    s.push(br#"{"limit":1"#).unwrap();
    assert_eq!(s.status("/limit"), Syntax::Incomplete);
    s.push(b"0").unwrap();
    assert_eq!(s.status("/limit"), Syntax::Incomplete);
    s.push(b"0").unwrap();
    assert_eq!(s.status("/limit"), Syntax::Incomplete);
    s.push(b"}").unwrap();
    assert_eq!(s.status("/limit"), Syntax::Complete);
    // ... and it was 100 all along, never 1 or 10.
    assert_eq!(
        s.snapshot()
            .unwrap()
            .child("limit")
            .map(|v| format!("{v:?}")),
        Some(r#"Number(Number("100"))"#.to_string())
    );
}

#[test]
fn literals_complete_on_their_last_byte_no_delimiter_needed() {
    let s = feed(&[r#"{"a":tru"#]);
    assert_eq!(s.status("/a"), Syntax::Incomplete);
    let s = feed(&[r#"{"a":true"#]);
    assert_eq!(s.status("/a"), Syntax::Complete);
}

#[test]
fn containers_complete_only_at_their_bracket() {
    let s = feed(&[r#"{"a":{"b":1}"#]);
    assert_eq!(s.status("/a"), Syntax::Complete);
    assert_eq!(s.status(""), Syntax::Incomplete); // root still open
    let s = feed(&[r#"{"a":{"b":1}}"#]);
    assert_eq!(s.status(""), Syntax::Complete);
}

#[test]
fn a_dangling_escape_contributes_nothing_to_the_stable_prefix() {
    let s = feed(&[r#"{"a":"x\"#]);
    assert_eq!(
        s.snapshot().unwrap().child("a"),
        Some(&Value::PartialString("x".into()))
    );
    // and a half-written \u likewise
    let s = feed(&[r#"{"a":"x\u00"#]);
    assert_eq!(
        s.snapshot().unwrap().child("a"),
        Some(&Value::PartialString("x".into()))
    );
    // once it resolves, it appears
    let s = feed(&[r#"{"a":"xA"#]);
    assert_eq!(
        s.snapshot().unwrap().child("a"),
        Some(&Value::PartialString("xA".into()))
    );
}

#[test]
fn a_split_multibyte_char_is_withheld_until_complete() {
    // "é" is two bytes; feeding only the first must not publish half of it.
    let e = "é".as_bytes();
    let mut s = Stream::new();
    s.push(br#"{"a":"x"#).unwrap();
    s.push(&e[..1]).unwrap();
    assert_eq!(
        s.snapshot().unwrap().child("a"),
        Some(&Value::PartialString("x".into())),
        "half a character must not be published"
    );
    s.push(&e[1..]).unwrap();
    assert_eq!(
        s.snapshot().unwrap().child("a"),
        Some(&Value::PartialString("xé".into()))
    );
}

#[test]
fn chunk_boundaries_are_irrelevant() {
    let doc = r#"{"a":"é😀A\n","b":[1,-2.5e3,true,null],"c":{"d":{}}}"#;
    let whole = feed(&[doc]).snapshot().unwrap();
    for split in 1..doc.len() {
        // Only split on char boundaries for the &str slicing; the parser is
        // byte-resumable and the per-byte case is covered above.
        if !doc.is_char_boundary(split) {
            continue;
        }
        let parts = feed(&[&doc[..split], &doc[split..]]).snapshot().unwrap();
        assert_eq!(parts, whole, "split at {split} changed the result");
    }
}

#[test]
fn byte_at_a_time_matches_all_at_once() {
    let doc = r#"{"a":"é😀","b":[1,2,{"c":true}]}"#;
    let whole = feed(&[doc]).snapshot().unwrap();
    let mut s = Stream::new();
    for b in doc.as_bytes() {
        s.push(&[*b]).unwrap();
    }
    assert_eq!(s.snapshot().unwrap(), whole);
}

#[test]
fn completion_is_irrevocable_even_when_the_document_later_fails() {
    // DESIGN §3.3: the guarantee is about the value, not the document.
    let mut s = Stream::new();
    s.push(br#"{"query":"x","#).unwrap();
    assert_eq!(s.status("/query"), Syntax::Complete);
    assert!(s.push(b"]").is_err()); // mismatched close kills the stream
    assert_eq!(
        s.status("/query"),
        Syntax::Complete,
        "a completed value stays completed after the document fails"
    );
}

#[test]
fn missing_is_distinct_from_incomplete() {
    let s = feed(&[r#"{"a":"x"#]);
    assert_eq!(s.status("/a"), Syntax::Incomplete);
    assert_eq!(s.status("/nope"), Syntax::Missing);
}

#[test]
fn nested_paths_and_array_indices() {
    let s = feed(&[r#"{"messages":[{"role":"user"},{"role":"assi"#]);
    assert_eq!(s.status("/messages/0/role"), Syntax::Complete);
    assert_eq!(s.status("/messages/0"), Syntax::Complete);
    assert_eq!(s.status("/messages/1/role"), Syntax::Incomplete);
    assert_eq!(s.status("/messages/1"), Syntax::Incomplete);
    assert_eq!(s.status("/messages"), Syntax::Incomplete);
    assert_eq!(s.status("/messages/2"), Syntax::Missing);
}

#[test]
fn pointer_escapes() {
    let s = feed(&[r#"{"a/b":1,"c~d":2}"#]);
    assert_eq!(s.status("/a~1b"), Syntax::Complete);
    assert_eq!(s.status("/c~0d"), Syntax::Complete);
}

#[test]
fn a_failed_stream_stays_failed() {
    let mut s = Stream::new();
    assert!(s.push(br#"{"a" 1}"#).is_err());
    assert!(s.push(br#"more"#).is_err());
    assert!(s.error().is_some());
}
