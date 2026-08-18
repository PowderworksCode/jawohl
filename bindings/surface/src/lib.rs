//! jawohl's surface for [jedem](https://github.com/PowderworksCode/jedem).
//!
//! This is the only hand-written file in `bindings/`. The Python and Node
//! crates beside it — manifests, module shims, build script and all — are
//! generated from what is here.
//!
//! # What this covers, and what it does not
//!
//! jedem v1 exposes **functions over plain values**: no handles, no streams, no
//! callbacks. jawohl's own API is a stateful [`Stream`], which is a handle and
//! therefore not yet expressible. So this is a deliberately batch-shaped
//! adapter: each function takes the bytes received so far and answers one
//! question about them.
//!
//! That is a real limitation. Every call re-parses from the beginning, because
//! without a handle there is nowhere to keep the parser between calls. For a
//! tool call of a few hundred bytes that costs nothing; for a long document it
//! is quadratic. The incremental API reaches these languages when jedem grows
//! handles.
//!
//! What it does deliver today is the part that matters most: **a caller in any
//! language can ask whether the generation it is paying for can still
//! succeed.**

use std::error::Error;

use jawohl::Stream;

/// How far along a value is.
///
/// A mirror of [`jawohl::Syntax`], because `#[derive(jedem::Enum)]` has to be
/// applied where the type is defined and jawohl itself takes no dependency on
/// jedem. The cost is one `From` impl; the gain is that Python receives a real
/// enum member and TypeScript a string-literal union, rather than a bare string
/// only convention keeps correct.
#[derive(jedem::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syntax {
    /// No value at this path yet.
    Missing,
    /// Present, but still arriving.
    Incomplete,
    /// Finished. It will not change, so it is safe to act on.
    Complete,
}

impl From<jawohl::Syntax> for Syntax {
    fn from(v: jawohl::Syntax) -> Self {
        match v {
            jawohl::Syntax::Missing => Self::Missing,
            jawohl::Syntax::Incomplete => Self::Incomplete,
            jawohl::Syntax::Complete => Self::Complete,
        }
    }
}

/// What is known about a value's validity.
///
/// A mirror of [`jawohl::Validation`], for the same reason as [`Syntax`].
#[derive(jedem::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Validation {
    /// Too incomplete to judge.
    Pending,
    /// Everything judgeable so far holds, but the value is unfinished.
    ValidSoFar,
    /// Complete, and every constraint holds.
    Valid,
    /// Complete, and some constraint fails.
    Invalid,
    /// Some constraint fails and no continuation can repair it — stop.
    IrrecoverablyInvalid,
}

impl From<jawohl::Validation> for Validation {
    fn from(v: jawohl::Validation) -> Self {
        match v {
            jawohl::Validation::Pending => Self::Pending,
            jawohl::Validation::ValidSoFar => Self::ValidSoFar,
            jawohl::Validation::Valid => Self::Valid,
            jawohl::Validation::Invalid => Self::Invalid,
            jawohl::Validation::IrrecoverablyInvalid => Self::IrrecoverablyInvalid,
        }
    }
}

fn stream_of(input: &str) -> Result<Stream, jawohl::ParseError> {
    let mut s = Stream::new();
    s.push(input.as_bytes())?;
    Ok(s)
}

fn validated(schema: &str, input: &str) -> Result<Stream, Box<dyn Error>> {
    // Two different error types, one signature. jedem never inspects the error
    // type — every backend raises with its `Display` text — so `Box<dyn Error>`
    // is enough and no `.map_err(|e| e.to_string())` is needed.
    let mut s = Stream::from_json_schema(schema)?;
    s.push(input.as_bytes())?;
    Ok(s)
}

/// jawohl, as functions over plain values.
#[jedem::export]
pub mod jawohl_api {
    use super::{stream_of, validated, Syntax, Validation};
    use std::error::Error;

    /// Complete a truncated JSON document so that it parses.
    ///
    /// Fails if the input is not a prefix of any valid JSON document.
    pub fn complete_json(input: &str) -> Result<String, jawohl::ParseError> {
        jawohl::complete_json(input)
    }

    /// Is this a prefix of some valid JSON document?
    pub fn is_valid_prefix(input: &str) -> bool {
        jawohl::Stream::new().push(input.as_bytes()).is_ok()
    }

    /// Has the document finished?
    pub fn is_complete(input: &str) -> Result<bool, jawohl::ParseError> {
        Ok(stream_of(input)?.is_document_complete())
    }

    /// How far along the value at `pointer` is.
    ///
    /// [`Syntax::Complete`] carries the stability guarantee: that value will
    /// not change, so it is safe to act on.
    pub fn status(input: &str, pointer: &str) -> Result<Syntax, jawohl::ParseError> {
        Ok(stream_of(input)?.status(pointer).into())
    }

    /// The completed document, but only once the value at `pointer` is final.
    /// Absent while it can still change.
    pub fn settled(input: &str, pointer: &str) -> Result<Option<String>, jawohl::ParseError> {
        let s = stream_of(input)?;
        if s.status(pointer) != jawohl::Syntax::Complete {
            return Ok(None);
        }
        Ok(Some(jawohl::complete_json(input)?))
    }

    /// Validate what has arrived against a JSON Schema.
    pub fn validate(
        schema: &str,
        input: &str,
        pointer: &str,
    ) -> Result<Validation, Box<dyn Error>> {
        Ok(validated(schema, input)?.validation(pointer).into())
    }

    /// **Can this generation still succeed?**
    ///
    /// `true` means no continuation of the input can satisfy the schema, so a
    /// caller should stop generating. This is the one function worth crossing a
    /// language boundary for.
    pub fn is_irrecoverable(schema: &str, input: &str) -> Result<bool, Box<dyn Error>> {
        Ok(validated(schema, input)?.is_irrecoverable())
    }

    /// Which schema constraints jawohl could not lower, and why. Empty when
    /// everything compiled.
    pub fn lowering_report(schema: &str) -> Result<String, Box<dyn Error>> {
        let s = jawohl::Stream::from_json_schema(schema)?;
        Ok(s.lowering_report()
            .map(|r| r.to_string())
            .unwrap_or_default())
    }
}

jedem::surface! { name: "jawohl", version: "0.2.0", api: [jawohl_api] }

#[cfg(test)]
mod tests {
    /// The drift guard: the committed bindings must match the surface.
    ///
    /// Covers every file jedem writes — manifests and shims too, not just the
    /// binding source — and every target, so adding a backend without a guard
    /// fails here.
    #[test]
    fn the_committed_bindings_match_the_surface() {
        for &target in jedem::Target::ALL {
            let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("..")
                .join(target.dir_name());
            for file in jedem::generate_crate(
                super::JEDEM_SURFACE,
                target,
                "jawohl_surface",
                "../surface",
                &format!("jawohl-{}", target.dir_name()),
            ) {
                let path = dir.join(&file.path);
                let committed = std::fs::read_to_string(&path)
                    .unwrap_or_else(|e| panic!("{} is missing: {e}", path.display()));
                assert_eq!(
                    committed,
                    file.contents,
                    "\n\nbindings/{}/{} is out of date.\nrun: cargo jedem generate\n",
                    target.dir_name(),
                    file.path
                );
            }
        }
    }
}
