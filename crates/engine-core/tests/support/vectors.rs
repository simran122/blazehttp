//! Test-vector loader for engine-core integration tests. Format and semantics: `docs/notes/vectors.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;

const VECTORS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors");
const INDEX_FILE: &str = "index.json";

/// One vector case: the bytes the server sends before closing, and what the engine must report.
#[derive(Debug)]
pub struct Case {
    pub name: String,
    pub input: Vec<u8>,
    pub method: String,
    pub expect: Expect,
}

/// The native engine's expected result; `Error` holds `ErrorCode`'s display form, e.g. `PROTOCOL/malformed-response`.
#[derive(Debug, PartialEq, Eq)]
pub enum Expect {
    Ok {
        status: u16,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
        keep_alive: bool,
    },
    Error(String),
}

#[derive(Deserialize)]
struct RawFile {
    cases: Vec<RawCase>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawCase {
    name: String,
    input: String,
    input_encoding: String,
    request: RawRequest,
    expect: RawExpect,
}

#[derive(Deserialize)]
struct RawRequest {
    method: String,
}

#[derive(Deserialize)]
struct RawExpect {
    native: RawNative,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum RawNative {
    Ok(RawOk),
    Error(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawOk {
    status: u16,
    headers: Vec<(String, String)>,
    body: String,
    keep_alive: bool,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

fn decode(raw: RawCase) -> Case {
    assert_eq!(
        raw.input_encoding, "utf8",
        "{}: unsupported inputEncoding",
        raw.name
    );
    let expect = match raw.expect.native {
        RawNative::Ok(ok) => Expect::Ok {
            status: ok.status,
            headers: ok.headers,
            body: ok.body.into_bytes(),
            keep_alive: ok.keep_alive,
        },
        RawNative::Error(error) => Expect::Error(error),
    };
    Case {
        name: raw.name,
        input: raw.input.into_bytes(),
        method: raw.request.method,
        expect,
    }
}

/// Every case of every vector file registered for `suite` in `tests/vectors/index.json`, paired with
/// the file's path relative to `tests/vectors`.
///
/// # Panics
///
/// As [`load_suite_in`].
#[must_use]
pub fn load_suite(suite: &str) -> Vec<(String, Case)> {
    load_suite_in(Path::new(VECTORS_ROOT), suite)
}

/// [`load_suite`] for the vectors directory `root`.
///
/// # Panics
///
/// When `index.json` or a vector file cannot be read or parsed, when `suite` has no files, when a
/// file has no cases, or when a `.json` file sits in a directory of the suite's files without being
/// registered in `index.json`.
#[must_use]
pub fn load_suite_in(root: &Path, suite: &str) -> Vec<(String, Case)> {
    let index: BTreeMap<String, Vec<String>> = read_json(&root.join(INDEX_FILE));
    let files: Vec<&String> = index
        .iter()
        .filter(|(_, suites)| suites.iter().any(|s| s == suite))
        .map(|(file, _)| file)
        .collect();
    assert!(
        !files.is_empty(),
        "suite {suite} has no files in {INDEX_FILE}"
    );
    let dirs: BTreeSet<&str> = files
        .iter()
        .map(|file| file.rsplit_once('/').map_or("", |(dir, _)| dir))
        .collect();
    for dir in dirs {
        let entries = fs::read_dir(root.join(dir)).unwrap_or_else(|e| panic!("{dir}: {e}"));
        for entry in entries {
            let path = entry.unwrap_or_else(|e| panic!("{dir}: {e}")).path();
            // Case-sensitive, like `listVectorFiles` in tools/vectors/schema.ts.
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_else(|| panic!("{}: non-UTF-8 file name", path.display()));
            let file = if dir.is_empty() {
                name.to_owned()
            } else {
                format!("{dir}/{name}")
            };
            assert!(
                file == INDEX_FILE || index.contains_key(&file),
                "{file} is not registered in tests/vectors/{INDEX_FILE}"
            );
        }
    }
    files
        .into_iter()
        .flat_map(|file| {
            let raw: RawFile = read_json(&root.join(file));
            assert!(!raw.cases.is_empty(), "{file}: no cases");
            raw.cases
                .into_iter()
                .map(move |case| (file.clone(), decode(case)))
        })
        .collect()
}
