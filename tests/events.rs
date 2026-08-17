//! The event log and its ordering guarantees.

use jawohl::{Event, Stream, Value, ValueKind};

fn drain(chunks: &[&str]) -> Vec<Event> {
    let mut s = Stream::new();
    let mut all = Vec::new();
    for c in chunks {
        s.push(c.as_bytes()).unwrap();
        all.extend(s.changes());
    }
    all
}

fn started(events: &[Event]) -> Vec<(&str, ValueKind)> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ValueStarted { path, kind } => Some((path.as_str(), *kind)),
            _ => None,
        })
        .collect()
}

fn completed(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::ValueCompleted { path, .. } => Some(path.as_str()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_scalar_document_starts_and_completes() {
    let ev = drain(&["42 "]);
    assert_eq!(started(&ev), [("", ValueKind::Number)]);
    assert_eq!(completed(&ev), [""]);
    assert!(matches!(ev.last(), Some(Event::DocumentCompleted)));
}

#[test]
fn paths_are_json_pointers() {
    let ev = drain(&[r#"{"a":{"b":[1,2]}}"#]);
    assert_eq!(
        started(&ev),
        [
            ("", ValueKind::Object),
            ("/a", ValueKind::Object),
            ("/a/b", ValueKind::Array),
            ("/a/b/0", ValueKind::Number),
            ("/a/b/1", ValueKind::Number),
        ]
    );
}

#[test]
fn pointer_tokens_are_escaped() {
    let ev = drain(&[r#"{"a/b":1,"c~d":2}"#]);
    assert_eq!(
        started(&ev).iter().map(|(p, _)| *p).collect::<Vec<_>>(),
        ["", "/a~1b", "/c~0d"]
    );
}

// ---- the ordering guarantees ----------------------------------------------

#[test]
fn guarantee_1_a_paths_events_are_totally_ordered() {
    // started before progressed before completed, for the same path.
    let ev = drain(&[r#"{"q":"ab"#, r#"cd""#, "}"]);
    let idx = |pred: fn(&Event) -> bool| ev.iter().position(pred).unwrap();
    let start = idx(|e| matches!(e, Event::ValueStarted { path, .. } if path == "/q"));
    let prog = idx(|e| matches!(e, Event::ValueProgressed { path, .. } if path == "/q"));
    let done = idx(|e| matches!(e, Event::ValueCompleted { path, .. } if path == "/q"));
    assert!(start < prog, "started must precede progressed");
    assert!(prog < done, "progressed must precede completed");
}

#[test]
fn guarantee_2_a_child_completes_before_its_parent() {
    let ev = drain(&[r#"{"a":{"b":{"c":1}}}"#]);
    // innermost first, root last
    assert_eq!(completed(&ev), ["/a/b/c", "/a/b", "/a", ""]);
}

#[test]
fn guarantee_3_document_completed_is_last_and_exactly_once() {
    for doc in [
        r#"{"a":1}"#,
        r#"[1,[2,[3]]]"#,
        r#""just a string""#,
        r#"null"#,
    ] {
        let ev = drain(&[doc]);
        let n = ev
            .iter()
            .filter(|e| matches!(e, Event::DocumentCompleted))
            .count();
        assert_eq!(n, 1, "{doc}: DocumentCompleted must appear exactly once");
        assert!(
            matches!(ev.last(), Some(Event::DocumentCompleted)),
            "{doc}: DocumentCompleted must be last"
        );
    }
}

#[test]
fn no_document_completed_while_the_document_is_open() {
    let ev = drain(&[r#"{"a":[1,2"#]);
    assert!(!ev.iter().any(|e| matches!(e, Event::DocumentCompleted)));
}

// ---- progress semantics ----------------------------------------------------

#[test]
fn progress_is_emitted_once_per_push_not_once_per_byte() {
    let mut s = Stream::new();
    s.push(br#"{"q":"abcdefghij"#).unwrap();
    let ev = s.changes();
    let progressed: Vec<_> = ev
        .iter()
        .filter(|e| matches!(e, Event::ValueProgressed { .. }))
        .collect();
    assert_eq!(
        progressed.len(),
        1,
        "one push over ten characters is one event"
    );
    assert!(matches!(
        progressed[0],
        Event::ValueProgressed { stable_prefix, .. } if stable_prefix == "abcdefghij"
    ));
}

#[test]
fn progress_carries_the_stable_prefix_only() {
    // A dangling escape contributes nothing, so the push that adds it emits no
    // progress at all.
    let mut s = Stream::new();
    s.push(br#"{"q":"ab"#).unwrap();
    let first: Vec<_> = s.changes();
    assert!(first.iter().any(|e| matches!(
        e, Event::ValueProgressed { stable_prefix, .. } if stable_prefix == "ab"
    )));

    s.push(br#"\"#).unwrap();
    assert!(
        !s.changes()
            .iter()
            .any(|e| matches!(e, Event::ValueProgressed { .. })),
        "an unresolved escape must not advance the stable prefix"
    );

    s.push(b"n").unwrap();
    assert!(s.changes().iter().any(|e| matches!(
        e, Event::ValueProgressed { stable_prefix, .. } if stable_prefix == "ab\n"
    )));
}

#[test]
fn progress_is_not_emitted_for_keys() {
    let mut s = Stream::new();
    s.push(br#"{"partial_key"#).unwrap();
    assert!(
        !s.changes()
            .iter()
            .any(|e| matches!(e, Event::ValueProgressed { .. })),
        "a key is not a value"
    );
}

#[test]
fn a_split_multibyte_char_does_not_advance_the_prefix() {
    let e = "é".as_bytes();
    let mut s = Stream::new();
    s.push(br#"{"q":"x"#).unwrap();
    let _ = s.changes();
    s.push(&e[..1]).unwrap();
    assert!(
        !s.changes()
            .iter()
            .any(|e| matches!(e, Event::ValueProgressed { .. })),
        "half a character is not stable"
    );
    s.push(&e[1..]).unwrap();
    assert!(s.changes().iter().any(|e| matches!(
        e, Event::ValueProgressed { stable_prefix, .. } if stable_prefix == "xé"
    )));
}

// ---- completed values ------------------------------------------------------

#[test]
fn completed_carries_the_value() {
    let ev = drain(&[r#"{"a":[1,true,null,"s"]}"#]);
    let by_path = |p: &str| {
        ev.iter().find_map(|e| match e {
            Event::ValueCompleted { path, value } if path == p => Some(value.clone()),
            _ => None,
        })
    };
    assert_eq!(by_path("/a/1"), Some(Value::Bool(true)));
    assert_eq!(by_path("/a/2"), Some(Value::Null));
    assert_eq!(by_path("/a/3"), Some(Value::String("s".into())));
    assert!(matches!(by_path("/a"), Some(Value::Array(v)) if v.len() == 4));
}

#[test]
fn a_number_completes_only_once_delimited() {
    let mut s = Stream::new();
    s.push(br#"{"n":10"#).unwrap();
    assert!(
        !s.changes().iter().any(|e| matches!(
            e, Event::ValueCompleted { path, .. } if path == "/n"
        )),
        "10 may still become 100"
    );
    s.push(b"}").unwrap();
    let ev = s.changes();
    assert!(ev.iter().any(|e| matches!(
        e, Event::ValueCompleted { path, value: Value::Number(n) }
        if path == "/n" && n.as_str() == "10"
    )));
}

// ---- draining --------------------------------------------------------------

#[test]
fn draining_empties_the_log() {
    let mut s = Stream::new();
    s.push(br#"{"a":1}"#).unwrap();
    assert!(!s.changes().is_empty());
    assert!(s.changes().is_empty(), "a second drain sees nothing");
}

#[test]
fn events_do_not_depend_on_chunk_boundaries() {
    // The same document, split every possible way, must yield the same
    // Started/Completed sequence. Progress events legitimately differ, since
    // they are coalesced per push.
    let doc = r#"{"a":[1,"two",{"b":null}],"c":true}"#;
    let baseline: Vec<Event> = drain(&[doc])
        .into_iter()
        .filter(|e| !matches!(e, Event::ValueProgressed { .. }))
        .collect();
    for split in 0..=doc.len() {
        if !doc.is_char_boundary(split) {
            continue;
        }
        let got: Vec<Event> = drain(&[&doc[..split], &doc[split..]])
            .into_iter()
            .filter(|e| !matches!(e, Event::ValueProgressed { .. }))
            .collect();
        assert_eq!(got, baseline, "split at {split} changed the event sequence");
    }
}

#[test]
fn path_accessor() {
    let ev = drain(&[r#"{"a":1}"#]);
    assert_eq!(ev[0].path(), Some(""));
    assert_eq!(
        ev.last().unwrap().path(),
        None,
        "DocumentCompleted has no path"
    );
}
