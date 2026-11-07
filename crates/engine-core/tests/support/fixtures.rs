//! Shared byte fixtures (`tests/fixtures/`), read by the integration tests and by `examples/h1_parse_bench.rs`
//! (included there with `#[path]`), so both use exactly the same bytes.

use std::path::Path;

/// The raw head `tests/fixtures/h1-heads/<name>`.
///
/// # Panics
///
/// When the file cannot be read.
#[must_use]
pub fn h1_head(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/h1-heads")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}
