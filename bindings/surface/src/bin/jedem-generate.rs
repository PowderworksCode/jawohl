//! The whole of jawohl's binding build step.
//!
//!     cargo jedem generate

jedem::generator_main! {
    surface: jawohl_surface::JEDEM_SURFACE,
    core: "jawohl_surface",
    core_dir: "../surface",
    out: "..",
}
