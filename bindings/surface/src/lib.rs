//! jawohl's surface for [jedem](https://github.com/PowderworksCode/jedem).
//!
//! jedem v1 exposes **functions over plain values** — no handles, no streams,
//! no callbacks. jawohl's own API is mostly a stateful `Stream`, which is a
//! handle and therefore not yet expressible. So this crate is a deliberately
//! batch-shaped adapter: each function takes the bytes received so far and
//! answers one question about them.
//!
//! That is a real limitation, not a presentation choice. Every call re-parses
//! from the beginning, because without a handle there is nowhere to keep the
//! parser between calls. For a tool call of a few hundred bytes that is
//! nothing; for a long document it is quadratic. The streaming API arrives in
//! other languages when jedem grows handles — see the README.
//!
//! What it demonstrates today is the part that matters most: **a caller in any
//! language can ask whether the generation it is paying for can still
//! succeed.**

use jawohl::{Stream, Syntax, Validation};

/// jawohl, as plain values.
pub struct Jawohl;

fn stream_of(input: &str) -> Result<Stream, jawohl::ParseError> {
    let mut s = Stream::new();
    s.push(input.as_bytes())?;
    Ok(s)
}

fn describe(v: Validation) -> String {
    match v {
        Validation::Pending => "pending",
        Validation::ValidSoFar => "valid_so_far",
        Validation::Valid => "valid",
        Validation::Invalid => "invalid",
        Validation::IrrecoverablyInvalid => "irrecoverably_invalid",
    }
    .to_string()
}

#[jedem::export]
impl Jawohl {
    /// Complete a truncated JSON document so that it parses.
    ///
    /// Raises if the input is not a prefix of any valid JSON document.
    pub fn complete_json(input: &str) -> Result<String, jawohl::ParseError> {
        jawohl::complete_json(input)
    }

    /// Is this a prefix of some valid JSON document?
    pub fn is_valid_prefix(input: &str) -> bool {
        Stream::new().push(input.as_bytes()).is_ok()
    }

    /// Has the document finished?
    pub fn is_complete(input: &str) -> Result<bool, jawohl::ParseError> {
        Ok(stream_of(input)?.is_document_complete())
    }

    /// How far along the value at `pointer` is: `missing`, `incomplete` or
    /// `complete`.
    ///
    /// `complete` carries the stability guarantee — that value will not change,
    /// so it is safe to act on.
    pub fn status(input: &str, pointer: &str) -> Result<String, jawohl::ParseError> {
        Ok(match stream_of(input)?.status(pointer) {
            Syntax::Missing => "missing",
            Syntax::Incomplete => "incomplete",
            Syntax::Complete => "complete",
        }
        .to_string())
    }

    /// The value at `pointer`, but only once it is final. Absent while it can
    /// still change.
    pub fn settled_value(input: &str, pointer: &str) -> Result<Option<String>, jawohl::ParseError> {
        let s = stream_of(input)?;
        if s.status(pointer) != Syntax::Complete {
            return Ok(None);
        }
        // The completed document with everything else stripped is overkill for
        // a demo; the completion of the prefix is what a caller displays.
        Ok(Some(jawohl::complete_json(input)?))
    }

    /// Validate what has arrived against a JSON Schema, returning one of
    /// `pending`, `valid_so_far`, `valid`, `invalid`, `irrecoverably_invalid`.
    pub fn validate(schema: &str, input: &str, pointer: &str) -> Result<String, String> {
        let mut s = Stream::from_json_schema(schema).map_err(|e| e.to_string())?;
        s.push(input.as_bytes()).map_err(|e| e.to_string())?;
        Ok(describe(s.validation(pointer)))
    }

    /// **Can this generation still succeed?**
    ///
    /// `true` means no continuation of the input can satisfy the schema, so a
    /// caller should stop generating. This is the one function worth crossing a
    /// language boundary for.
    pub fn is_irrecoverable(schema: &str, input: &str) -> Result<bool, String> {
        let mut s = Stream::from_json_schema(schema).map_err(|e| e.to_string())?;
        s.push(input.as_bytes()).map_err(|e| e.to_string())?;
        Ok(s.is_irrecoverable())
    }

    /// Which schema constraints jawohl could not lower, and why. Empty when
    /// everything compiled.
    pub fn lowering_report(schema: &str) -> Result<String, String> {
        let s = Stream::from_json_schema(schema).map_err(|e| e.to_string())?;
        Ok(s.lowering_report()
            .map(|r| r.to_string())
            .unwrap_or_default())
    }
}

jedem::surface! { name: "jawohl", version: "0.2.0", api: [Jawohl] }

#[cfg(test)]
mod tests {
    /// The drift guard: the committed bindings must match the surface.
    ///
    /// jedem serialises nothing, so there is no interchange file to go stale --
    /// but the generated bindings are committed, and those can.
    #[test]
    fn the_committed_bindings_match_the_surface() {
        for (target, committed, path) in [
            (
                jedem::Target::Python,
                include_str!("../../python/src/generated.rs"),
                "bindings/python/src/generated.rs",
            ),
            (
                jedem::Target::Node,
                include_str!("../../node/src/generated.rs"),
                "bindings/node/src/generated.rs",
            ),
        ] {
            let fresh = jedem::generate(super::JEDEM_SURFACE, target, "jawohl_surface");
            assert_eq!(
                committed, fresh,
                "\n\n{path} is out of date.\nrun: cargo run -p jawohl-surface --bin generate\n"
            );
        }
        assert_eq!(
            jedem::Target::ALL.len(),
            2,
            "a new jedem target needs a drift guard here"
        );
    }
}
