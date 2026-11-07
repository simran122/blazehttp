//! Response vector cases (`tests/vectors/h1-response/`): the bytes a server sends before closing, and what the
//! engine must report. Format: `docs/notes/vectors.md`.

use serde::Deserialize;

use super::vectors::{VectorCase, bytes};

/// One response vector case.
#[derive(Debug)]
pub struct Case {
    pub name: String,
    pub input: Vec<u8>,
    pub method: String,
    pub expect: Expect,
}

/// The native engine's expected result; `Error` holds `ErrorCode`'s display form, e.g. `PROTOCOL/malformed-response`.
/// Header names, values and the body are bytes, decoded with the case's `inputEncoding`.
#[derive(Debug, PartialEq, Eq)]
pub enum Expect {
    Ok {
        status: u16,
        headers: Vec<(Vec<u8>, Vec<u8>)>,
        body: Vec<u8>,
        keep_alive: bool,
    },
    Error(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawCase {
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

impl VectorCase for Case {
    type Raw = RawCase;

    fn decode(raw: RawCase) -> Self {
        let encoding = raw.input_encoding.as_str();
        let name = raw.name.as_str();
        let expect = match raw.expect.native {
            RawNative::Ok(ok) => Expect::Ok {
                status: ok.status,
                headers: ok
                    .headers
                    .into_iter()
                    .map(|(n, v)| (bytes(n, encoding, name), bytes(v, encoding, name)))
                    .collect(),
                body: bytes(ok.body, encoding, name),
                keep_alive: ok.keep_alive,
            },
            RawNative::Error(error) => Expect::Error(error),
        };
        Self {
            input: bytes(raw.input, encoding, name),
            method: raw.request.method,
            expect,
            name: raw.name,
        }
    }
}
