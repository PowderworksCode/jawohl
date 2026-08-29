//! The ten-case table from notes/DESIGN.md §1 — the argument for the rewrite.
//!
//! 1.0 emitted output that does not parse for six of these, and returned `Ok`
//! for all six. Every case here asserts the completion both parses and says
//! what it should.

use jawohl::complete_json;

fn done(input: &str) -> String {
    complete_json(input).unwrap_or_else(|e| panic!("{input:?}: {e}"))
}

/// Every completion must be parseable JSON.
fn parses(s: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(s).is_ok()
}

#[test]
fn the_six_cases_1_0_corrupted() {
    // A partial literal is finished, not left dangling.
    assert_eq!(done(r#"{"a": tru"#), r#"{"a": true}"#);
    // A trailing backslash is dropped — 1.0 let it escape the quote it added.
    assert_eq!(done(r#"{"a": "x\"#), r#"{"a": "x"}"#);
    // A truncated \u escape contributes nothing.
    assert_eq!(done(r#"{"a": "x\u00"#), r#"{"a": "x"}"#);
    // A key with no value is dropped entirely.
    assert_eq!(done(r#"{"a":"#), r#"{}"#);
    // A trailing comma is dropped.
    assert_eq!(done(r#"{"a":1,"#), r#"{"a":1}"#);
    // A partial key is not a member yet.
    assert_eq!(done(r#"{"que"#), r#"{}"#);
}

#[test]
fn all_ten_cases_parse() {
    for case in [
        r#"{"a": tru"#,
        r#"{"a": "x\"#,
        r#"{"a": "x\u00"#,
        r#"{"a":"#,
        r#"{"a":1,"#,
        r#"{"que"#,
        r#"{"limit": 10"#,
        r#"{"query":"rust par"#,
        r#"{"k":"v","arr":[1,2,{"n":"v"#,
        r#"{"a": "he said \"hi"#,
    ] {
        let out = done(case);
        assert!(
            parses(&out),
            "{case:?} completed to {out:?}, which does not parse"
        );
    }
}

#[test]
fn the_four_cases_1_0_already_handled() {
    assert_eq!(done(r#"{"limit": 10"#), r#"{"limit": 10}"#);
    assert_eq!(done(r#"{"query":"rust par"#), r#"{"query":"rust par"}"#);
    assert_eq!(
        done(r#"{"k":"v","arr":[1,2,{"n":"v"#),
        r#"{"k":"v","arr":[1,2,{"n":"v"}]}"#
    );
    assert_eq!(done(r#"{"a": "he said \"hi"#), r#"{"a": "he said \"hi"}"#);
}

#[test]
fn malformed_input_is_an_error_not_garbage() {
    // 1.0 returned Ok for every one of these.
    for bad in [
        r#"{"a": 1}}"#,    // unbalanced close
        r#"[1,2}"#,        // mismatched close
        r#"{"a" 1}"#,      // missing colon
        r#"{"a": tru}"#,   // literal that cannot finish
        r#"{"a": 01}"#,    // leading zero
        r#"{"a": .5}"#,    // no integer part
        r#"{} {}"#,        // trailing content
        r#"{"a": "x\q"}"#, // bad escape
    ] {
        assert!(
            complete_json(bad).is_err(),
            "{bad:?} should be rejected, got {:?}",
            complete_json(bad)
        );
    }
}

#[test]
fn complete_documents_round_trip_unchanged() {
    for good in [
        r#"{ "spaced" : [ 1 , 2 ] }"#,
        r#"{}"#,
        r#"[]"#,
        r#"{"a":1,"b":[true,false,null],"c":{"d":"e"}}"#,
        r#"[0,-1,1.5,1e10,1E+10,-0.5e-3]"#,
        r#""é😀""#,
    ] {
        assert_eq!(
            done(good),
            good,
            "a complete document must be returned untouched"
        );
    }
}

#[test]
fn empty_and_whitespace() {
    assert_eq!(done(""), "");
    assert_eq!(done("   "), "");
}
