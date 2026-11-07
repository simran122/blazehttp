//! Test-vector loader for engine-core integration tests. Format and semantics: `docs/notes/vectors.md`.
//! One loader walks `tests/vectors/index.json` for every suite; each case kind (`response_vectors.rs`,
//! `request_vectors.rs`) only says how its JSON decodes. The informational fields `reference` and `derivedFrom` are
//! validated by `tools/vectors/schema.ts` and not read here.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde::de::DeserializeOwned;

const VECTORS_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tests/vectors");
const INDEX_FILE: &str = "index.json";

/// A case kind: the raw JSON shape of one case and how it becomes the decoded case.
pub trait VectorCase: Sized {
    type Raw: DeserializeOwned;

    fn decode(raw: Self::Raw) -> Self;
}

#[derive(Deserialize)]
#[serde(bound = "R: DeserializeOwned")]
struct RawFile<R> {
    cases: Vec<R>,
}

fn read_json<T: DeserializeOwned>(path: &Path) -> T {
    let text = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// `utf8`: the string's UTF-8 bytes; `latin1`: one byte per code point U+0000–U+00FF.
pub fn bytes(text: String, encoding: &str, case: &str) -> Vec<u8> {
    match encoding {
        "utf8" => text.into_bytes(),
        "latin1" => text
            .chars()
            .map(|c| {
                u8::try_from(u32::from(c))
                    .unwrap_or_else(|_| panic!("{case}: {c:?} is not a latin1 byte"))
            })
            .collect(),
        other => panic!("{case}: unsupported inputEncoding {other}"),
    }
}

/// Every case of every vector file registered for `suite` in `tests/vectors/index.json`, paired with
/// the file's path relative to `tests/vectors`.
///
/// # Panics
///
/// As [`load_suite_in`].
#[must_use]
pub fn load_suite<C: VectorCase>(suite: &str) -> Vec<(String, C)> {
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
pub fn load_suite_in<C: VectorCase>(root: &Path, suite: &str) -> Vec<(String, C)> {
    suite_files(root, suite)
        .into_iter()
        .flat_map(|file| {
            let raw: RawFile<C::Raw> = read_json(&root.join(&file));
            assert!(!raw.cases.is_empty(), "{file}: no cases");
            raw.cases
                .into_iter()
                .map(move |case| (file.clone(), C::decode(case)))
        })
        .collect()
}

/// The files registered for `suite`, after checking that every `.json` file next to them is registered.
fn suite_files(root: &Path, suite: &str) -> Vec<String> {
    let index: BTreeMap<String, Vec<String>> = read_json(&root.join(INDEX_FILE));
    let files: Vec<String> = index
        .iter()
        .filter(|(_, suites)| suites.iter().any(|s| s == suite))
        .map(|(file, _)| file.clone())
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
        assert_registered(root, dir, &index);
    }
    files
}

/// Panics on a `.json` file in `root/dir` that `index` does not list.
fn assert_registered(root: &Path, dir: &str, index: &BTreeMap<String, Vec<String>>) {
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
