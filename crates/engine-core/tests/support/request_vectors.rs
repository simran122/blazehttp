//! Request vector cases (`tests/vectors/h1-request/`): the parts of one request, its header template and the exact
//! bytes the writer must produce, or its error. Format: `docs/notes/vectors.md`.

use serde::Deserialize;

use super::vectors::{VectorCase, bytes};

/// One request vector case; every byte string is decoded with the case's `inputEncoding`.
#[derive(Debug)]
pub struct RequestCase {
    pub name: String,
    pub method: String,
    pub target: Vec<u8>,
    pub template_host: Vec<u8>,
    pub template_fields: Vec<u8>,
    pub headers: Vec<u8>,
    pub header_lines: u16,
    pub body: Vec<u8>,
    /// The connection's authority; `templateHost` when the case does not set it.
    pub conn_authority: Vec<u8>,
    pub expect: RequestExpect,
}

/// The writer's expected result; `Error` holds `ErrorCode`'s display form, e.g. `ENGINE/internal`.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestExpect {
    /// The whole request (head and body) and `allowed` or `closeAfterExchange`.
    Written {
        bytes: Vec<u8>,
        reuse: String,
    },
    Error(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawRequestCase {
    name: String,
    input_encoding: String,
    request: RawRequest,
    expect: RawExpect,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRequest {
    method: String,
    target: String,
    template_host: String,
    template_fields: String,
    headers: String,
    header_lines: u16,
    body: String,
    conn_authority: Option<String>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawExpect {
    Written { bytes: String, reuse: String },
    Error { error: String },
}

impl VectorCase for RequestCase {
    type Raw = RawRequestCase;

    fn decode(raw: RawRequestCase) -> Self {
        let encoding = raw.input_encoding.as_str();
        let name = raw.name.as_str();
        let decode = |text: String| bytes(text, encoding, name);
        let request = raw.request;
        let template_host = decode(request.template_host);
        let expect = match raw.expect {
            RawExpect::Written { bytes, reuse } => RequestExpect::Written {
                bytes: decode(bytes),
                reuse,
            },
            RawExpect::Error { error } => RequestExpect::Error(error),
        };
        Self {
            method: request.method,
            target: decode(request.target),
            conn_authority: request
                .conn_authority
                .map_or_else(|| template_host.clone(), decode),
            template_host,
            template_fields: decode(request.template_fields),
            headers: decode(request.headers),
            header_lines: request.header_lines,
            body: decode(request.body),
            expect,
            name: raw.name,
        }
    }
}
