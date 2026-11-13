mod support {
    pub mod request_vectors;
    pub mod response_vectors;
    pub mod vectors;
}

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use engine_core::generated::spec::{ErrorCode, Method, NetworkDetail};
use support::request_vectors::{RequestCase, RequestExpect, RequestReuse};
use support::response_vectors::{ResponseCase, ResponseExpect};
use support::vectors::{load_suite, load_suite_in};

const SUITE: &str = "engine-core:h1_response";
const REQUEST_SUITE: &str = "engine-core:h1_writer";

/// Every generated method name.
fn generated_methods() -> BTreeSet<&'static str> {
    (0..)
        .map_while(Method::from_wire)
        .map(Method::as_str)
        .collect()
}

/// Every error the engine can produce, as `ErrorCode`'s `Display` prints it.
fn engine_errors() -> BTreeSet<String> {
    ErrorCode::ALL_SAMPLES
        .iter()
        .map(ToString::to_string)
        .collect()
}

#[test]
fn h1_response_suite_loads_with_requests_the_engine_knows() {
    let cases = load_suite::<ResponseCase>(SUITE);
    assert!(!cases.is_empty());
    let methods = generated_methods();
    for (file, case) in &cases {
        assert!(
            methods.contains(case.method.as_str()),
            "{file}/{}: unknown method {}",
            case.name,
            case.method
        );
    }
}

#[test]
fn only_an_empty_input_closes_before_the_response() {
    let closed = ErrorCode::Network(NetworkDetail::ClosedBeforeResponse).to_string();
    for (file, case) in load_suite::<ResponseCase>(SUITE) {
        let expects_closed = matches!(&case.expect, ResponseExpect::Error(e) if *e == closed);
        assert_eq!(
            case.input.is_empty(),
            expects_closed,
            "{file}/{}: an exchange is closed before the response exactly when no byte arrives",
            case.name
        );
    }
}

#[test]
fn expected_errors_are_errors_the_engine_can_produce() {
    let known = engine_errors();
    for (file, case) in load_suite::<ResponseCase>(SUITE) {
        if let ResponseExpect::Error(error) = &case.expect {
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
    let cases = load_suite_in::<ResponseCase>(dir.root(), "s");
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
    let _ = load_suite_in::<ResponseCase>(dir.root(), "s");
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
    let _ = load_suite_in::<ResponseCase>(dir.root(), "s");
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
    let _ = load_suite_in::<ResponseCase>(dir.root(), "s");
}

#[test]
fn h1_writer_suite_loads_with_known_methods_errors_and_reuse() {
    let methods = generated_methods();
    let errors = engine_errors();
    let cases = load_suite::<RequestCase>(REQUEST_SUITE);
    assert!(!cases.is_empty());
    for (file, case) in &cases {
        let at = format!("{file}/{}", case.name);
        assert!(methods.contains(case.method.as_str()), "{at}: method");
        match &case.expect {
            RequestExpect::Written { bytes, .. } => assert!(!bytes.is_empty(), "{at}: bytes"),
            RequestExpect::Error(error) => assert!(errors.contains(error), "{at}: {error}"),
        }
    }
}

const ONE_REQUEST_CASE: &str = r#"{ "cases": [{ "name": "a", "inputEncoding": "latin1",
    "request": { "method": "GET", "target": "/", "templateHost": "h", "templateFields": "", "templateFieldLines": 0, "headers": "x: \u00e9\r\n",
    "headerLines": 1, "body": "" },
    "expect": { "bytes": "GET / HTTP/1.1\r\nhost: h\r\nx: \u00e9\r\n\r\n", "reuse": "allowed" } }] }"#;

#[test]
fn request_cases_decode_latin1_and_default_the_connection_authority() {
    let dir = TempVectors::new(
        "request",
        &[
            ("index.json", r#"{ "h1-request/a.json": ["s"] }"#),
            ("h1-request/a.json", ONE_REQUEST_CASE),
        ],
    );
    let cases = load_suite_in::<RequestCase>(dir.root(), "s");
    let [(_, case)] = cases.as_slice() else {
        panic!("one case expected");
    };
    assert_eq!(
        (
            case.name.as_str(),
            case.method.as_str(),
            case.target.as_slice()
        ),
        ("a", "GET", &b"/"[..])
    );
    assert_eq!(
        (
            case.template_host.as_slice(),
            case.template_fields.as_slice()
        ),
        (&b"h"[..], &b""[..])
    );
    assert_eq!(case.template_field_lines, 0);
    assert_eq!(case.headers, b"x: \xe9\r\n");
    assert_eq!((case.header_lines, case.body.as_slice()), (1, &b""[..]));
    assert_eq!(case.conn_authority, b"h");
    assert_eq!(
        case.expect,
        RequestExpect::Written {
            bytes: b"GET / HTTP/1.1\r\nhost: h\r\nx: \xe9\r\n\r\n".to_vec(),
            reuse: RequestReuse::Allowed,
        }
    );
}

/// Loads [`ONE_REQUEST_CASE`] with `from` replaced by `to`; panics as the loader does.
fn load_edited_request_case(test: &str, from: &str, to: &str) {
    let edited = ONE_REQUEST_CASE.replacen(from, to, 1);
    assert_ne!(edited, ONE_REQUEST_CASE, "{test}: the edit must apply");
    let dir = TempVectors::new(
        test,
        &[
            ("index.json", r#"{ "h1-request/a.json": ["s"] }"#),
            ("h1-request/a.json", &edited),
        ],
    );
    let _ = load_suite_in::<RequestCase>(dir.root(), "s");
}

#[test]
#[should_panic(expected = "h1-request/a.json")]
fn request_expect_with_both_bytes_and_error_panics() {
    load_edited_request_case(
        "request-both",
        r#""reuse": "allowed" }"#,
        r#""reuse": "allowed", "error": "ENGINE/internal" }"#,
    );
}

#[test]
#[should_panic(expected = "h1-request/a.json")]
fn request_null_connection_authority_panics() {
    load_edited_request_case(
        "request-null-authority",
        r#""body": "" }"#,
        r#""body": "", "connAuthority": null }"#,
    );
}

#[test]
#[should_panic(expected = "h1-request/a.json")]
fn request_null_derived_from_panics() {
    load_edited_request_case(
        "request-null-derived-from",
        r#""inputEncoding": "latin1","#,
        r#""inputEncoding": "latin1", "derivedFrom": null,"#,
    );
}
