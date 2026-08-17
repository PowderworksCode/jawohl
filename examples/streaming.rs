//! Level 2: watch a document materialise, one chunk at a time.
//!
//!     cargo run --example streaming
//!
//! Shows the two things that make this more than a parser: values report when
//! they are *final*, and the change log says what happened rather than handing
//! you a snapshot to diff.

use jawohl::{Event, Stream, Syntax};

fn main() {
    // Roughly how a tool call arrives from a model: a few tokens at a time.
    let chunks = [
        r#"{"name":"search"#,
        r#"_web","arguments":{"#,
        r#""query":"rust incremental"#,
        r#" parser","limit":1"#,
        r#"0}}"#,
    ];

    let mut s = Stream::new();
    for (i, chunk) in chunks.iter().enumerate() {
        s.push(chunk.as_bytes()).expect("valid prefix");
        println!("chunk {i}: {chunk:?}");
        for event in s.changes() {
            match event {
                Event::ValueStarted { path, kind } => {
                    println!("    started    {path:28} {kind:?}")
                }
                Event::ValueProgressed {
                    path,
                    stable_prefix,
                } => {
                    println!("    progressed {path:28} {stable_prefix:?}")
                }
                Event::ValueCompleted { path, .. } => {
                    println!("    COMPLETED  {path:28} (will not change)")
                }
                Event::DocumentCompleted => println!("    document complete"),
                other => println!("    {other:?}"),
            }
        }
    }

    println!();
    // The stability guarantee in one line: `limit` was `1` before the final
    // chunk, and reporting it complete then would have been wrong.
    println!("/arguments/query -> {:?}", s.status("/arguments/query"));
    println!("/arguments/limit -> {:?}", s.status("/arguments/limit"));
    assert_eq!(s.status("/arguments/limit"), Syntax::Complete);
}
