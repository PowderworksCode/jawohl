//! The realistic shape: JSON arriving inside a server-sent event stream.
//!
//!     cargo run --example sse
//!
//! Every major provider delivers structured output this way -- `data:` lines
//! carrying fragments of one JSON document. jawohl does not parse SSE itself
//! (see the framing note in the README), so this shows the four lines of glue,
//! and what you get for them.

use jawohl::{Event, Stream, Validation};

/// A recorded stream, of the shape a provider actually sends.
const WIRE: &str = "\
event: message\n\
data: {\"tool\":\"send_ema\n\
\n\
data: il\",\"args\":{\"to\":\"a@b.c\n\
\n\
data: om\",\"subject\":\"Hi\",\n\
\n\
data: \"body\":\"...\"}}\n\
\n\
data: [DONE]\n\
\n";

const SCHEMA: &str = r#"{
  "type": "object",
  "properties": {
    "tool": {"enum": ["send_email", "search_web"]},
    "args": {"type": "object", "required": ["to"]}
  }
}"#;

fn main() {
    let mut s = Stream::from_json_schema(SCHEMA).expect("schema compiles");

    for line in WIRE.lines() {
        let Some(payload) = line.strip_prefix("data: ") else {
            continue; // event:, id:, retry:, and blank separators
        };
        if payload == "[DONE]" {
            break;
        }
        s.push(payload.as_bytes()).expect("valid prefix");

        for e in s.changes() {
            if let Event::ValueCompleted { path, value } = e {
                // Safe to act on: this value can no longer change.
                println!("final  {path:16} = {value:?}");
            }
        }
    }

    println!();
    println!("document      -> {:?}", s.validation(""));
    println!("/tool         -> {:?}", s.validation("/tool"));
    assert_eq!(s.validation("/tool"), Validation::Valid);

    // The point of `ValueCompleted`: /tool was final long before the document
    // was, so a dispatcher could have started resolving the tool while the
    // arguments were still arriving.
}
