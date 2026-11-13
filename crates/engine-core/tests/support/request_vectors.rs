//! Request vector cases (`tests/vectors/h1-request/`): the parts of one request, its header template and the exact
//! bytes the writer must produce, or its error. Format: `docs/notes/vectors.md`.

use engine_core::h1::Reuse;
use serde::{Deserialize, Deserializer};

use super::vectors::{VectorCase, bytes};

/// One request vector case; every byte string is decoded with the case's `inputEncoding`.
#[derive(Debug)]
pub struct RequestCase {
    pub name: String,
    pub method: String,
    pub target: Vec<u8>,
    pub template_host: Vec<u8>,
    pub template_fields: Vec<u8>,
    /// The field count JS serialised into `template_fields`.
    pub template_field_lines: u16,
    pub headers: Vec<u8>,
    pub header_lines: u16,
    pub body: Vec<u8>,
    /// The connection's authority; `templateHost` when the case does not set it.
    pub conn_authority: Vec<u8>,
    pub expect: RequestExpect,
}

/// `reuse` as the vectors spell it; equal to the writer's [`Reuse`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RequestReuse {
    Allowed,
    CloseAfterExchange,
}

impl From<Reuse> for RequestReuse {
    fn from(reuse: Reuse) -> Self {
        match reuse {
            Reuse::Allowed => Self::Allowed,
            Reuse::CloseAfterExchange => Self::CloseAfterExchange,
        }
    }
}

/// The writer's expected result; `Error` holds `ErrorCode`'s display form, e.g. `ENGINE/internal`.
#[derive(Debug, PartialEq, Eq)]
pub enum RequestExpect {
    /// The whole request (head and body).
    Written {
        bytes: Vec<u8>,
        reuse: RequestReuse,
    },
    Error(String),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RawRequestCase {
    name: String,
    input_encoding: String,
    request: RawRequest,
    expect: RawExpect,
    #[serde(default, rename = "derivedFrom")]
    _derived_from: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RawRequest {
    method: String,
    target: String,
    template_host: String,
    template_fields: String,
    template_field_lines: u16,
    headers: String,
    header_lines: u16,
    body: String,
    /// Absent means "the template host"; `null` is rejected, as `tools/vectors/schema.ts` does.
    #[serde(default, deserialize_with = "present_string")]
    conn_authority: Option<String>,
}

fn present_string<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    String::deserialize(deserializer).map(Some)
}

/// Each shape denies unknown fields, so an `expect` mixing `bytes` and `error` matches neither.
#[derive(Deserialize)]
#[serde(untagged)]
enum RawExpect {
    Written(RawWritten),
    Error(RawError),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWritten {
    bytes: String,
    reuse: RequestReuse,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawError {
    error: String,
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
            RawExpect::Written(RawWritten { bytes, reuse }) => RequestExpect::Written {
                bytes: decode(bytes),
                reuse,
            },
            RawExpect::Error(RawError { error }) => RequestExpect::Error(error),
        };
        Self {
            method: request.method,
            target: decode(request.target),
            conn_authority: request
                .conn_authority
                .map_or_else(|| template_host.clone(), decode),
            template_host,
            template_fields: decode(request.template_fields),
            template_field_lines: request.template_field_lines,
            headers: decode(request.headers),
            header_lines: request.header_lines,
            body: decode(request.body),
            expect,
            name: raw.name,
        }
    }
}
