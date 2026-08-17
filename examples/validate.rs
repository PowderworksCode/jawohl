//! Level 3: cancel a generation that cannot succeed.
//!
//!     cargo run --example validate
//!
//! The payoff of incremental validation is not knowing sooner that output was
//! wrong -- it is not paying for the rest of it.

use jawohl::Stream;

const SCHEMA: &str = r#"{
  "type": "object",
  "required": ["role", "limit"],
  "properties": {
    "role":  {"enum": ["user", "admin"]},
    "limit": {"type": "integer", "maximum": 100},
    "query": {"type": "string", "minLength": 3, "maxLength": 20}
  },
  "additionalProperties": false
}"#;

/// Feed one byte at a time and stop the moment the document is doomed.
fn stream_until_doomed(label: &str, input: &str) {
    let mut s = Stream::from_json_schema(SCHEMA).expect("schema compiles");
    for (i, b) in input.bytes().enumerate() {
        if s.push(&[b]).is_err() {
            println!("{label}: parse error at byte {i}");
            return;
        }
        if s.is_irrecoverable() {
            println!(
                "{label}: CANCELLED after {} of {} bytes -- {:?}",
                i + 1,
                input.len(),
                &input[..=i]
            );
            return;
        }
    }
    println!(
        "{label}: read all {} bytes -> {:?}",
        input.len(),
        s.validation("")
    );
}

fn main() {
    // Rejected before the string even closes: no member begins "sup".
    stream_until_doomed(
        "bad role  ",
        r#"{"role":"support","limit":10,"query":"rust"}"#,
    );

    // Rejected before the number is delimited: under the default PlainDecimal
    // profile, 1000 can only grow.
    stream_until_doomed(
        "bad limit ",
        r#"{"role":"user","limit":1000,"query":"rust"}"#,
    );

    // Rejected the moment an unknown key completes.
    stream_until_doomed("extra key ", r#"{"role":"user","nope":1,"limit":5}"#);

    // Rejected on the 21st character of a 20-character maximum.
    stream_until_doomed(
        "long query",
        r#"{"role":"user","limit":5,"query":"aaaaaaaaaaaaaaaaaaaaaaaaaa"}"#,
    );

    // Valid all the way through.
    stream_until_doomed(
        "good      ",
        r#"{"role":"admin","limit":50,"query":"rust"}"#,
    );

    println!();
    // Constraints jawohl could not lower are reported, never silently skipped.
    let s = Stream::from_json_schema(r#"{"type":"string","if":{},"pattern":"foo"}"#).unwrap();
    print!("{}", s.lowering_report().unwrap());
}
