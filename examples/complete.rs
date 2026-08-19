//! Level 1: finish a truncated document so it parses.
//!
//!     cargo run --example complete

use jawohl::complete_json;

fn main() {
    // Each of these is a prefix of a document a model was part-way through
    // emitting. The first six are cases the 1.0 bracket-counter got wrong,
    // returning `Ok` with output that does not parse.
    let prefixes = [
        r#"{"query":"rust par"#,
        r#"{"a": tru"#,
        r#"{"a": "x\"#,
        r#"{"a": "x\u00"#,
        r#"{"a":"#,
        r#"{"a":1,"#,
        r#"{"que"#,
        r#"{"k":"v","arr":[1,2,{"n":"v"#,
    ];

    for p in prefixes {
        match complete_json(p) {
            Ok(done) => println!("{p:32} -> {done}"),
            Err(e) => println!("{p:32} -> error: {e}"),
        }
    }

    // Malformed input is an error, not plausible-looking garbage.
    println!();
    for bad in [r#"{"a": 1}}"#, r#"{"a": 01}"#, r#"{} {}"#] {
        println!(
            "{bad:32} -> {:?}",
            complete_json(bad).unwrap_err().to_string()
        );
    }
}
