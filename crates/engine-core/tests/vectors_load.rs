mod support {
    pub mod vectors;
}

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use engine_core::generated::spec::{ErrorCode, Method};
use support::vectors::{Expect, load_suite, load_suite_in};

const SUITE: &str = "engine-core:h1_response";

#[test]
fn h1_response_suite_loads_with_requests_the_engine_knows() {
    let cases = load_suite(SUITE);
    assert!(!cases.is_empty());
    let methods: BTreeSet<&str> = (0..)
        .map_while(Method::from_wire)
        .map(Method::as_str)
        .collect();
    for (file, case) in &cases {
        assert!(!case.input.is_empty(), "{file}/{}: empty input", case.name);
        assert!(
            methods.contains(case.method.as_str()),
            "{file}/{}: unknown method {}",
            case.name,
            case.method
        );
    }
}

#[test]
fn expected_errors_are_errors_the_engine_can_produce() {
    let known: BTreeSet<String> = ErrorCode::ALL_SAMPLES
        .iter()
        .map(ToString::to_string)
        .collect();
    for (file, case) in load_suite(SUITE) {
        if let Expect::Error(error) = &case.expect {
            assert!(
                known.contains(error),
                "{file}/{}: {error} is not an ErrorCode display form",
                case.name
            );
        }
    }
}

/// A vectors directory under the system temp dir, removed when dropped (also while a test unwinds).
struct TempVectors(PathBuf);

impl TempVectors {
    fn new(test: &str, files: &[(&str, &str)]) -> Self {
        let root = std::env::temp_dir().join(format!("blazehttp-{test}-{}", std::process::id()));
        for (path, text) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("file has a parent")).expect("create dir");
            fs::write(path, text).expect("write file");
        }
        Self(root)
    }

    fn root(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempVectors {
    fn drop(&mut self) {
        // Best-effort cleanup; a leftover temp dir must not mask the test result.
        let _ = fs::remove_dir_all(&self.0);
    }
}

const ONE_CASE: &str = r#"{ "cases": [{ "name": "a", "input": "x", "inputEncoding": "utf8",
    "request": { "method": "GET" }, "expect": { "native": { "error": "PROTOCOL/malformed-response" } } }] }"#;

#[test]
fn loads_registered_files_from_any_root() {
    let dir = TempVectors::new(
        "ok",
        &[
            ("index.json", r#"{ "h1/a.json": ["s"] }"#),
            ("h1/a.json", ONE_CASE),
            (
                "h1/notes.JSON",
                "not a vector file: extensions match case-sensitively",
            ),
        ],
    );
    let cases = load_suite_in(dir.root(), "s");
    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].0, "h1/a.json");
}

#[test]
#[should_panic(expected = "h1/b.json is not registered")]
fn unregistered_file_panics() {
    let dir = TempVectors::new(
        "unregistered",
        &[
            ("index.json", r#"{ "h1/a.json": ["s"] }"#),
            ("h1/a.json", ONE_CASE),
            ("h1/b.json", ONE_CASE),
        ],
    );
    let _ = load_suite_in(dir.root(), "s");
}

#[test]
#[should_panic(expected = "suite s has no files")]
fn empty_suite_panics() {
    let dir = TempVectors::new(
        "empty-suite",
        &[
            ("index.json", r#"{ "a.json": ["other"] }"#),
            ("a.json", ONE_CASE),
        ],
    );
    let _ = load_suite_in(dir.root(), "s");
}

#[test]
#[should_panic(expected = "a.json: no cases")]
fn file_without_cases_panics() {
    let dir = TempVectors::new(
        "no-cases",
        &[
            ("index.json", r#"{ "a.json": ["s"] }"#),
            ("a.json", r#"{ "cases": [] }"#),
        ],
    );
    let _ = load_suite_in(dir.root(), "s");
}
