//! Regenerate both bindings from the annotated surface.
//!
//!     cargo run -p jawohl-surface --bin generate

fn main() -> std::io::Result<()> {
    for (target, dir) in [
        (jedem::Target::Python, "python"),
        (jedem::Target::Node, "node"),
    ] {
        let path = format!("{}/../{}/src/generated.rs", env!("CARGO_MANIFEST_DIR"), dir);
        let code = jedem::generate(jawohl_surface::JEDEM_SURFACE, target, "jawohl_surface");
        std::fs::write(&path, &code)?;
        println!("wrote {path} ({} bytes)", code.len());
    }
    Ok(())
}
