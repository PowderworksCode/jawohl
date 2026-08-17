//! Incremental validation: what a prefix already proves.

use jawohl::{Event, NumberProfile, Stream, Validation};

fn feed(schema: &str, chunks: &[&str]) -> Stream {
    let mut s = Stream::from_json_schema(schema).expect("schema should compile");
    for c in chunks {
        s.push(c.as_bytes()).expect("input should parse");
    }
    s
}

// ---- the design's two motivating examples ---------------------------------

#[test]
fn an_enum_is_rejected_before_the_string_closes() {
    // DESIGN section 4.1: no member begins "sup", and none ever will.
    let s = feed(
        r#"{"properties":{"role":{"enum":["user","admin"]}}}"#,
        [r#"{"role":"sup"#].as_ref(),
    );
    assert_eq!(s.validation("/role"), Validation::IrrecoverablyInvalid);
    assert!(
        s.is_irrecoverable(),
        "the document cannot be completed validly"
    );
}

#[test]
fn a_live_enum_prefix_stays_pending() {
    let s = feed(
        r#"{"properties":{"role":{"enum":["user","admin"]}}}"#,
        [r#"{"role":"us"#].as_ref(),
    );
    assert_eq!(s.validation("/role"), Validation::Pending);
    assert!(!s.is_irrecoverable());
}

#[test]
fn a_numeric_bound_is_rejected_before_the_number_is_delimited() {
    // DESIGN section 4.2: the motivating early-cancel case, which works only
    // under PlainDecimal.
    let s = feed(
        r#"{"properties":{"limit":{"maximum":100}}}"#,
        [r#"{"limit":1000"#].as_ref(),
    );
    assert_eq!(s.validation("/limit"), Validation::IrrecoverablyInvalid);
    assert!(s.is_irrecoverable());
}

#[test]
fn under_exact_no_numeric_bound_decides_early() {
    // Sound: 1000 may still become 1000e-9. Exact gives up the early verdict
    // rather than guessing.
    let mut s = Stream::from_json_schema(r#"{"properties":{"limit":{"maximum":100}}}"#)
        .unwrap()
        .with_number_profile(NumberProfile::Exact);
    s.push(br#"{"limit":1000"#).unwrap();
    assert_eq!(s.validation("/limit"), Validation::Pending);
    assert!(!s.is_irrecoverable());
    // and it is caught the moment the number is delimited
    s.push(b"}").unwrap();
    assert_eq!(s.validation("/limit"), Validation::Invalid);
}

// ---- the profile is enforced, not assumed ---------------------------------

#[test]
fn an_exponent_under_plain_decimal_fails_the_stream() {
    let mut s = Stream::from_json_schema(r#"{"properties":{"x":{"maximum":100}}}"#).unwrap();
    let e = s.push(br#"{"x":1e"#).unwrap_err();
    assert!(
        matches!(e.kind, jawohl::ParseErrorKind::NumberProfileViolated),
        "got {e:?}"
    );
    // The message must point at the way out, not just complain.
    let text = e.to_string();
    assert!(text.contains("Exact"), "got: {text}");
}

#[test]
fn an_exponent_under_exact_is_just_a_number() {
    let mut s = Stream::from_json_schema(r#"{"properties":{"x":{"maximum":1e9}}}"#)
        .unwrap()
        .with_number_profile(NumberProfile::Exact);
    s.push(br#"{"x":1e3}"#).unwrap();
    assert_eq!(s.validation("/x"), Validation::Valid);
}

#[test]
fn without_a_schema_an_exponent_is_never_a_violation() {
    // No verdict to be unsound about, so nothing to enforce.
    let mut s = Stream::new();
    s.push(br#"{"x":1e10}"#).unwrap();
    assert_eq!(s.validation("/x"), Validation::Pending);
}

// ---- string constraints ----------------------------------------------------

#[test]
fn max_length_seals_shut() {
    let schema = r#"{"properties":{"u":{"type":"string","maxLength":5}}}"#;
    let s = feed(schema, [r#"{"u":"abc"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::ValidSoFar);
    let s = feed(schema, [r#"{"u":"abcdefgh"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::IrrecoverablyInvalid);
}

#[test]
fn min_length_seals_open() {
    // DESIGN section 4: minLength is Pending until reached, then permanently
    // satisfied -- it can only become more true as the string grows.
    let schema = r#"{"properties":{"u":{"type":"string","minLength":3}}}"#;
    let s = feed(schema, [r#"{"u":"ab"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::Pending);
    let s = feed(schema, [r#"{"u":"abcd"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::ValidSoFar);
}

#[test]
fn the_designs_username_example() {
    // 3 <= length <= 20. "za" is pending on min and fine on max; twenty-six
    // letters is irrecoverable on max even though the string has not finished.
    let schema = r#"{"properties":{"u":{"type":"string","minLength":3,"maxLength":20}}}"#;
    let s = feed(schema, [r#"{"u":"za"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::Pending);

    let s = feed(schema, [r#"{"u":"abcdefghijklmnopqrstuvwxyz"#].as_ref());
    assert_eq!(s.validation("/u"), Validation::IrrecoverablyInvalid);
}

#[test]
fn an_anchored_pattern_rejects_early() {
    let schema = r#"{"properties":{"zip":{"pattern":"^[0-9]{5}$"}}}"#;
    let s = feed(schema, [r#"{"zip":"123"#].as_ref());
    assert_eq!(s.validation("/zip"), Validation::Pending);
    let s = feed(schema, [r#"{"zip":"12a"#].as_ref());
    assert_eq!(s.validation("/zip"), Validation::IrrecoverablyInvalid);
    let s = feed(schema, [r#"{"zip":"12345"}"#].as_ref());
    assert_eq!(s.validation("/zip"), Validation::Valid);
}

#[test]
fn an_unanchored_pattern_never_rejects_early() {
    // Sound: "zzz" can still become "zzzfoo".
    let schema = r#"{"properties":{"s":{"pattern":"foo"}}}"#;
    let s = feed(schema, [r#"{"s":"zzz"#].as_ref());
    assert_ne!(s.validation("/s"), Validation::IrrecoverablyInvalid);
    let s = feed(schema, [r#"{"s":"zzz"}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Invalid);
}

// ---- type, the earliest verdict of all -------------------------------------

#[test]
fn a_type_mismatch_is_irrecoverable_at_the_first_byte() {
    let schema = r#"{"properties":{"n":{"type":"string"}}}"#;
    let s = feed(schema, [r#"{"n":["#].as_ref());
    assert_eq!(s.validation("/n"), Validation::IrrecoverablyInvalid);
}

#[test]
fn integer_is_only_decidable_when_the_number_completes() {
    let schema = r#"{"properties":{"n":{"type":"integer"}}}"#;
    let s = feed(schema, [r#"{"n":1.5"#].as_ref());
    assert_ne!(
        s.validation("/n"),
        Validation::Invalid,
        "1.5 could still be 1.5e1"
    );
    let s = feed(schema, [r#"{"n":1.5}"#].as_ref());
    assert_eq!(s.validation("/n"), Validation::Invalid);
    let s = feed(schema, [r#"{"n":42}"#].as_ref());
    assert_eq!(s.validation("/n"), Validation::Valid);
}

// ---- objects and arrays ----------------------------------------------------

#[test]
fn required_is_pending_until_the_object_closes() {
    let schema = r#"{"type":"object","required":["a"],"properties":{"b":{"type":"string"}}}"#;
    let s = feed(schema, [r#"{"b":"x""#].as_ref());
    assert_eq!(
        s.validation(""),
        Validation::Pending,
        "`a` may still arrive"
    );
    let s = feed(schema, [r#"{"b":"x"}"#].as_ref());
    assert_eq!(s.validation(""), Validation::Invalid);
}

#[test]
fn an_unknown_key_is_irrecoverable_under_additional_properties_false() {
    let schema = r#"{"type":"object","properties":{"a":{}},"additionalProperties":false}"#;
    let s = feed(schema, [r#"{"nope":1"#].as_ref());
    assert_eq!(s.validation(""), Validation::IrrecoverablyInvalid);
}

#[test]
fn max_items_seals_shut_and_min_items_seals_open() {
    let schema = r#"{"properties":{"a":{"type":"array","maxItems":2,"minItems":1}}}"#;
    let s = feed(schema, [r#"{"a":[1"#].as_ref());
    assert_eq!(s.validation("/a"), Validation::ValidSoFar);
    let s = feed(schema, [r#"{"a":[1,2,3"#].as_ref());
    assert_eq!(s.validation("/a"), Validation::IrrecoverablyInvalid);
}

#[test]
fn unique_items_is_irrecoverable_on_the_first_settled_duplicate() {
    let schema = r#"{"properties":{"a":{"type":"array","uniqueItems":true}}}"#;

    // Not yet a duplicate: the trailing 1 is undelimited and may still become
    // 10. The stability guarantee and uniqueItems interact exactly here, and
    // calling it a duplicate now would be a verdict a later byte could refute.
    let s = feed(schema, [r#"{"a":[1,2,1"#].as_ref());
    assert_ne!(s.validation("/a"), Validation::IrrecoverablyInvalid);

    // Delimited, so the duplicate is settled and can never be undone.
    let s = feed(schema, [r#"{"a":[1,2,1,"#].as_ref());
    assert_eq!(s.validation("/a"), Validation::IrrecoverablyInvalid);

    // A string settles at its closing quote, with no delimiter needed.
    let s = feed(schema, [r#"{"a":["x","y","x""#].as_ref());
    assert_eq!(s.validation("/a"), Validation::IrrecoverablyInvalid);

    // And the escape hatch stays open while it could still diverge.
    let s = feed(schema, [r#"{"a":["x","y","x"#].as_ref());
    assert_ne!(s.validation("/a"), Validation::IrrecoverablyInvalid);
}

#[test]
fn element_schemas_apply_to_items() {
    let schema = r#"{"properties":{"a":{"type":"array","items":{"type":"string"}}}}"#;
    let s = feed(schema, [r#"{"a":["ok",1"#].as_ref());
    assert_eq!(s.validation("/a"), Validation::IrrecoverablyInvalid);
}

// ---- propagation -----------------------------------------------------------

#[test]
fn irrecoverability_propagates_up_to_the_root() {
    // DESIGN section 4.3: a dead child kills its parent, and a dead root is
    // the early-cancellation signal.
    let schema = r#"{"properties":{"outer":{"properties":{"inner":{"enum":["yes"]}}}}}"#;
    let s = feed(schema, [r#"{"outer":{"inner":"no"#].as_ref());
    assert_eq!(
        s.validation("/outer/inner"),
        Validation::IrrecoverablyInvalid
    );
    assert_eq!(s.validation("/outer"), Validation::IrrecoverablyInvalid);
    assert_eq!(s.validation(""), Validation::IrrecoverablyInvalid);
    assert!(s.is_irrecoverable());
}

#[test]
fn a_valid_document_reaches_valid_only_when_complete() {
    let schema = r#"{"type":"object","required":["a"],"properties":{"a":{"type":"string"}}}"#;
    let s = feed(schema, [r#"{"a":"x""#].as_ref());
    assert_ne!(s.validation(""), Validation::Valid, "still open");
    let s = feed(schema, [r#"{"a":"x"}"#].as_ref());
    assert_eq!(s.validation(""), Validation::Valid);
}

// ---- combinators -----------------------------------------------------------

#[test]
fn all_of_composes_incrementally() {
    // An intersection: any dead branch kills the whole, with no branch state
    // needed.
    let schema = r#"{"properties":{"s":{"allOf":[{"type":"string"},{"maxLength":3}]}}}"#;
    let s = feed(schema, [r#"{"s":"abcdef"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::IrrecoverablyInvalid);
}

#[test]
fn any_of_waits_for_completion() {
    // Deliberate: judging anyOf on a prefix needs per-branch state, so this
    // version is conservatively Pending rather than guessing.
    let schema = r#"{"properties":{"s":{"anyOf":[{"type":"string"},{"type":"integer"}]}}}"#;
    let s = feed(schema, [r#"{"s":"ab"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Pending);
    let s = feed(schema, [r#"{"s":"ab"}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Valid);
    let s = feed(schema, [r#"{"s":true}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Invalid);
}

#[test]
fn one_of_requires_exactly_one() {
    let schema = r#"{"properties":{"s":{"oneOf":[{"type":"string"},{"maxLength":100}]}}}"#;
    // A string satisfies both branches, so oneOf fails.
    let s = feed(schema, [r#"{"s":"ab"}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Invalid);
}

#[test]
fn not_inverts() {
    let schema = r#"{"properties":{"s":{"not":{"type":"integer"}}}}"#;
    let s = feed(schema, [r#"{"s":"ab"}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Valid);
    let s = feed(schema, [r#"{"s":5}"#].as_ref());
    assert_eq!(s.validation("/s"), Validation::Invalid);
}

// ---- events ----------------------------------------------------------------

#[test]
fn a_failure_is_an_event_not_an_error() {
    // Errors-as-events: the stream keeps going and the consumer decides.
    let mut s = Stream::from_json_schema(r#"{"properties":{"r":{"enum":["user"]}}}"#).unwrap();
    s.push(br#"{"r":"zz"#).unwrap(); // Ok, not Err
    let events = s.changes();
    let failed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::ValidationFailed { path, state } => Some((path.as_str(), *state)),
            _ => None,
        })
        .collect();
    assert!(
        failed.contains(&("/r", Validation::IrrecoverablyInvalid)),
        "expected a failure event for /r, got {failed:?}"
    );
}

#[test]
fn a_transition_is_reported_once_not_on_every_push() {
    let mut s = Stream::from_json_schema(r#"{"properties":{"r":{"enum":["user"]}}}"#).unwrap();
    s.push(br#"{"r":"zz"#).unwrap();
    let first = s.changes();
    assert!(first
        .iter()
        .any(|e| matches!(e, Event::ValidationFailed { .. })));

    s.push(b"z").unwrap();
    let second = s.changes();
    assert!(
        !second
            .iter()
            .any(|e| matches!(e, Event::ValidationFailed { path, .. } if path == "/r")),
        "still irrecoverable, but the state did not move: {second:?}"
    );
}

#[test]
fn validation_events_never_precede_the_value_starting() {
    let mut s = Stream::from_json_schema(r#"{"properties":{"r":{"enum":["user"]}}}"#).unwrap();
    s.push(br#"{"r":"zz"#).unwrap();
    let events = s.changes();
    let started = events
        .iter()
        .position(|e| matches!(e, Event::ValueStarted { path, .. } if path == "/r"));
    let failed = events
        .iter()
        .position(|e| matches!(e, Event::ValidationFailed { path, .. } if path == "/r"));
    assert!(started.is_some() && failed.is_some());
    assert!(started < failed, "ValueStarted must come first");
}

// ---- the report ------------------------------------------------------------

#[test]
fn the_lowering_report_is_reachable_from_the_stream() {
    let s = Stream::from_json_schema(r#"{"type":"string","if":{}}"#).unwrap();
    let report = s.lowering_report().expect("a schema was attached");
    assert!(report.unsupported.iter().any(|u| u.keyword == "if"));
}

#[test]
fn without_a_schema_there_is_no_report_and_no_verdict() {
    let s = Stream::new();
    assert!(s.lowering_report().is_none());
    assert_eq!(s.validation(""), Validation::Pending);
    assert!(!s.is_irrecoverable());
}

#[test]
fn a_path_the_schema_says_nothing_about_is_pending() {
    let s = feed(
        r#"{"properties":{"known":{"type":"string"}}}"#,
        [r#"{"unknown":123}"#].as_ref(),
    );
    assert_eq!(s.validation("/unknown"), Validation::Pending);
}

// ---- chunking must not change the verdict ----------------------------------

#[test]
fn verdicts_do_not_depend_on_chunk_boundaries() {
    let schema = r#"{"type":"object","required":["role"],"properties":{"role":{"enum":["user","admin"]},"limit":{"maximum":100}}}"#;
    for doc in [
        r#"{"role":"admin","limit":50}"#,
        r#"{"role":"nope","limit":50}"#,
        r#"{"limit":5}"#,
    ] {
        let whole = feed(schema, [doc].as_ref()).validation("");
        for split in 0..=doc.len() {
            if !doc.is_char_boundary(split) {
                continue;
            }
            let parts = feed(schema, [&doc[..split], &doc[split..]].as_ref()).validation("");
            assert_eq!(parts, whole, "{doc:?} split at {split} changed the verdict");
        }
    }
}
