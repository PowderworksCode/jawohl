//! Compiling JSON Schema into the constraint IR.

use jawohl::schema::{compile, AdditionalProperties, Monotonicity, TypeSet};

#[test]
fn a_flat_schema_lowers_every_keyword() {
    let s = compile(r#"{"type":"string","minLength":3,"maxLength":20}"#).unwrap();
    let n = s.root_node();
    assert_eq!(n.types, Some(TypeSet::STRING));
    assert_eq!(n.min_length, Some(3));
    assert_eq!(n.max_length, Some(20));
    assert_eq!(s.lowering_report().compiled, 3);
    assert!(s.lowering_report().unsupported.is_empty());
}

#[test]
fn boolean_schemas_are_schemas() {
    assert_eq!(compile("true").unwrap().root_node().boolean, Some(true));
    assert_eq!(compile("false").unwrap().root_node().boolean, Some(false));
}

#[test]
fn nested_properties_become_nodes() {
    let s = compile(
        r#"{"type":"object","properties":{"a":{"type":"integer"},"b":{"type":"array","items":{"type":"string"}}},"required":["a"]}"#,
    )
    .unwrap();
    let root = s.root_node();
    assert_eq!(root.required, ["a"]);
    let a = s.node(root.properties["a"]);
    assert_eq!(a.types, Some(TypeSet::INTEGER));
    let b = s.node(root.properties["b"]);
    let items = s.node(b.items.unwrap());
    assert_eq!(items.types, Some(TypeSet::STRING));
}

#[test]
fn additional_properties_false_is_recorded() {
    let s = compile(r#"{"type":"object","additionalProperties":false}"#).unwrap();
    assert!(matches!(
        s.root_node().additional_properties,
        AdditionalProperties::Forbidden
    ));
}

#[test]
fn combinators_lower_to_node_lists() {
    let s = compile(
        r#"{"allOf":[{"type":"string"}],"anyOf":[{"minLength":1},{"maxLength":5}],"oneOf":[true],"not":{"const":"x"}}"#,
    )
    .unwrap();
    let n = s.root_node();
    assert_eq!(n.all_of.len(), 1);
    assert_eq!(n.any_of.len(), 2);
    assert_eq!(n.one_of.len(), 1);
    assert!(n.not.is_some());
}

// ---- $ref, which Pydantic and Zod emit for every nested model --------------

#[test]
fn local_refs_resolve() {
    let s = compile(
        r##"{"type":"object","properties":{"a":{"$ref":"#/$defs/Name"}},"$defs":{"Name":{"type":"string","minLength":1}}}"##,
    )
    .unwrap();
    let a = s.node(s.root_node().properties["a"]);
    assert_eq!(a.types, Some(TypeSet::STRING));
    assert_eq!(a.min_length, Some(1));
}

#[test]
fn recursive_refs_terminate() {
    // {"$ref":"#"} is a cycle; the arena must reuse the node under
    // construction rather than expand forever.
    let s = compile(
        r##"{"type":"object","properties":{"child":{"$ref":"#"},"name":{"type":"string"}}}"##,
    )
    .unwrap();
    let root = s.root();
    let child = s.root_node().properties["child"];
    assert_eq!(child, root, "a self-reference must point back at the root");
    assert!(s.node_count() < 10, "recursion must not blow up the arena");
}

#[test]
fn mutually_recursive_refs_terminate() {
    let s = compile(
        r##"{"$ref":"#/$defs/A","$defs":{"A":{"type":"object","properties":{"b":{"$ref":"#/$defs/B"}}},"B":{"type":"object","properties":{"a":{"$ref":"#/$defs/A"}}}}}"##,
    )
    .unwrap();
    assert!(s.node_count() < 20);
}

#[test]
fn remote_refs_are_refused_not_fetched() {
    let e = compile(r#"{"$ref":"https://example.com/schema.json"}"#).unwrap_err();
    assert!(format!("{e}").contains("$ref"), "got {e}");
}

#[test]
fn an_unresolvable_local_ref_is_an_error() {
    assert!(compile(r##"{"$ref":"#/$defs/Nope"}"##).is_err());
}

// ---- the honesty contract --------------------------------------------------

#[test]
fn unsupported_keywords_are_reported_never_dropped() {
    let s = compile(
        r#"{"type":"object","unevaluatedProperties":false,"if":{"const":1},"patternProperties":{"^x":{}}}"#,
    )
    .unwrap();
    let names: Vec<&str> = s
        .lowering_report()
        .unsupported
        .iter()
        .map(|u| u.keyword.as_str())
        .collect();
    for expected in ["unevaluatedProperties", "if", "patternProperties"] {
        assert!(
            names.contains(&expected),
            "{expected} must be reported; got {names:?}"
        );
    }
}

#[test]
fn an_unrecognised_keyword_is_reported() {
    let s = compile(r#"{"type":"string","totallyMadeUp":1}"#).unwrap();
    assert!(s
        .lowering_report()
        .unsupported
        .iter()
        .any(|u| u.keyword == "totallyMadeUp"));
}

#[test]
fn annotations_are_not_reported_as_unsupported() {
    let s = compile(
        r#"{"$schema":"https://json-schema.org/draft/2020-12/schema","$id":"x","title":"T","description":"d","default":1,"examples":[1],"type":"string"}"#,
    )
    .unwrap();
    assert!(
        s.lowering_report().unsupported.is_empty(),
        "annotations carry no assertion: {:?}",
        s.lowering_report().unsupported
    );
}

#[test]
fn the_report_reads_like_the_design_says() {
    let s = compile(r#"{"type":"string","minLength":3,"maxLength":20,"if":{}}"#).unwrap();
    let text = s.lowering_report().to_string();
    assert!(text.contains("3 constraints compiled"), "got: {text}");
    assert!(text.contains("1 unsupported"), "got: {text}");
}

// ---- enum, the exactly-decidable case --------------------------------------

#[test]
fn enum_lowers_to_a_prefix_index() {
    let s = compile(r#"{"enum":["user","admin"]}"#).unwrap();
    let t = s.root_node().enumeration.as_ref().unwrap();
    // the design's own example: "sup" can never become a member
    assert!(t.prefix_is_live("us"));
    assert!(!t.prefix_is_live("sup"));
    assert!(t.contains("admin"));
}

#[test]
fn a_mixed_enum_remembers_it_has_non_strings() {
    let s = compile(r#"{"enum":["a",1,null]}"#).unwrap();
    let t = s.root_node().enumeration.as_ref().unwrap();
    assert!(t.has_non_strings());
    assert_eq!(t.string_members(), ["a"]);
}

// ---- pattern, where soundness needs an anchor ------------------------------

#[test]
fn an_anchored_pattern_supports_early_rejection() {
    let s = compile(r#"{"pattern":"^[a-z]+$"}"#).unwrap();
    let p = s.root_node().pattern.as_ref().unwrap();
    assert!(p.supports_early_rejection());
    assert!(p.prefix_is_live("abc"));
    assert!(!p.prefix_is_live("aB"));
    assert!(s.lowering_report().unsupported.is_empty());
}

#[test]
fn an_unanchored_pattern_says_so_in_the_report() {
    // Sound: any prefix can still be extended into a match, so no early
    // rejection is possible -- and the caller is told rather than left to
    // assume it got one.
    let s = compile(r#"{"pattern":"foo"}"#).unwrap();
    let p = s.root_node().pattern.as_ref().unwrap();
    assert!(!p.supports_early_rejection());
    assert!(p.prefix_is_live("zzz"));
    assert!(p.matches("a foo b"));
    assert!(s
        .lowering_report()
        .unsupported
        .iter()
        .any(|u| u.keyword == "pattern" && u.reason.contains("unanchored")));
}

#[test]
fn a_broken_regex_is_reported() {
    let s = compile(r#"{"pattern":"[unclosed"}"#).unwrap();
    assert!(s.root_node().pattern.is_none());
    assert!(s
        .lowering_report()
        .unsupported
        .iter()
        .any(|u| u.keyword == "pattern"));
}

// ---- monotonicity, the classification the evaluator dispatches on ----------

#[test]
fn numeric_bounds_are_only_decidable_at_completion() {
    // DESIGN section 4.2: 1000 may still become 1000e-9, so no numeric bound
    // is soundly decidable on a prefix in plain JSON.
    let s = compile(r#"{"maximum":100}"#).unwrap();
    assert_eq!(
        s.root_node().numeric_bound_monotonicity(),
        Monotonicity::AtCompletion
    );
}

// ---- malformed input -------------------------------------------------------

#[test]
fn a_schema_that_is_not_json_is_an_error() {
    assert!(compile("{not json").is_err());
}

#[test]
fn a_schema_that_is_not_an_object_is_an_error() {
    assert!(compile(r#""a string""#).is_err());
    assert!(compile("[1,2]").is_err());
}

#[test]
fn a_realistic_pydantic_style_schema_compiles_clean() {
    // The shape Pydantic emits for a nested model: $defs plus $ref.
    let s = compile(
        r##"{
          "$defs": {
            "Address": {
              "type": "object",
              "properties": {"city": {"type":"string","minLength":1}, "zip": {"type":"string","pattern":"^[0-9]{5}$"}},
              "required": ["city"],
              "additionalProperties": false
            }
          },
          "type": "object",
          "properties": {
            "name": {"type":"string","minLength":3,"maxLength":20},
            "age": {"type":"integer","minimum":0,"maximum":150},
            "role": {"enum":["user","admin"]},
            "address": {"$ref":"#/$defs/Address"},
            "tags": {"type":"array","items":{"type":"string"},"maxItems":10,"uniqueItems":true}
          },
          "required": ["name","role"],
          "additionalProperties": false
        }"##,
    )
    .unwrap();
    let unsupported = &s.lowering_report().unsupported;
    assert!(
        unsupported.is_empty(),
        "should lower cleanly, got {unsupported:?}"
    );
    let root = s.root_node();
    assert_eq!(root.required, ["name", "role"]);
    let addr = s.node(root.properties["address"]);
    assert_eq!(addr.required, ["city"]);
    let tags = s.node(root.properties["tags"]);
    assert_eq!(tags.max_items, Some(10));
    assert!(tags.unique_items);
}
