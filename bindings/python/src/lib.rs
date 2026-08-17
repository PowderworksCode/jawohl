//! The Python binding. Everything of substance is generated.
mod generated;

#[pyo3::pymodule]
fn jawohl(m: &pyo3::Bound<'_, pyo3::types::PyModule>) -> pyo3::PyResult<()> {
    generated::register(m)
}
