//! The benchmark cells: every response pre-built once, at start-up (design note §8).
//!
//! **Singleton** (design §13): `CATALOG` is a process-wide `LazyLock`, so the cells are built once per process and
//! every connection task shares the same immutable bytes.

use std::sync::LazyLock;

/// Heads shared with the parser tests and benchmark, so the parser is measured on the bytes the server sends.
const HEAD_1K: &[u8] = include_bytes!("../../../../tests/fixtures/h1-heads/1k.http");
const HEAD_1K_H20: &[u8] = include_bytes!("../../../../tests/fixtures/h1-heads/1k-h20.http");
/// The 847-byte JSON payload of every 1 KiB cell.
const PAYLOAD: &[u8] = include_bytes!("../../../../tests/fixtures/bodies/1k.json");
/// `PAYLOAD` gzipped once and committed, so the served bytes never depend on a compressor version.
const PAYLOAD_GZIP: &[u8] = include_bytes!("../../../../tests/fixtures/bodies/1k.json.gz");

const DATE: &str = "Fri, 06 Nov 2026 12:00:00 GMT";
const JSON: &str = "application/json";
const CHUNK_BYTES: usize = 256;
const ONE_MIB: usize = 1 << 20;

/// One benchmark cell: a request path and its complete pre-built response.
///
/// A passive value type, so its fields are public (design §13.2 rule 1 carve-out, Rust API Guidelines
/// C-STRUCT-PRIVATE): it holds no invariant beyond what [`cells`] builds.
#[derive(Debug)]
pub struct Cell {
    pub path: &'static str,
    /// Status line, head and framed body, exactly as written to the socket.
    pub response: Vec<u8>,
    /// Bytes of `response` up to and including the blank line: the answer to a `HEAD` request.
    pub head_len: usize,
    /// FNV-1a 64 of the decoded body (no chunked framing, no gzip): the bytes a client's application receives.
    pub body_fnv: u64,
    /// For a chunked cell, the response HTTP/1.0 clients get instead (they cannot receive chunked framing, RFC 9112
    /// §6.1): the same body, close-delimited.
    pub close_delimited: Option<CloseDelimited>,
}

/// A response whose body ends when the connection closes (no `content-length`, no `transfer-encoding`; RFC 9112 §6.3
/// rule 8). A passive value type with public fields (design §13.2 rule 1 carve-out).
#[derive(Debug)]
pub struct CloseDelimited {
    /// Status line, head and unframed body, exactly as written to the socket.
    pub response: Vec<u8>,
    /// Bytes of `response` up to and including the blank line: the answer to a `HEAD` request.
    pub head_len: usize,
}

/// The bytes to send for a request, and whether the connection must close after them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Served {
    pub(crate) bytes: &'static [u8],
    pub(crate) close: bool,
}

/// Why a request is answered with an error that closes the connection: at once for a 400 or 431, after the request's
/// declared body for a 505.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    /// 400: a request head this fixture does not accept.
    BadRequest,
    /// 431: a request head larger than the read buffer.
    HeadTooLarge,
    /// 505: an HTTP major version other than 1.
    VersionNotSupported,
}

struct Catalog {
    cells: Vec<Cell>,
    not_found: Vec<u8>,
    bad_request: Vec<u8>,
    head_too_large: Vec<u8>,
    version_not_supported: Vec<u8>,
}

static CATALOG: LazyLock<Catalog> = LazyLock::new(Catalog::build);

/// All cells, built on first use (`serve` forces this before it reports ready).
#[must_use]
pub fn cells() -> &'static [Cell] {
    &CATALOG.cells
}

/// Builds every response now, so the first request does not pay for it.
pub(crate) fn prebuild() {
    LazyLock::force(&CATALOG);
}

/// The pre-built response for a request target (only its head for `HEAD`); unknown targets get a pre-built 404. An
/// HTTP/1.0 request for a chunked cell gets the close-delimited variant, and the connection closes after it.
pub(crate) fn response_for(target: &[u8], head_only: bool, http10: bool) -> Served {
    let keep = |bytes: &'static [u8]| Served {
        bytes,
        close: false,
    };
    let Some(cell) = CATALOG
        .cells
        .iter()
        .find(|cell| cell.path.as_bytes() == target)
    else {
        return keep(&CATALOG.not_found);
    };
    match &cell.close_delimited {
        Some(variant) if http10 => {
            let bytes = if head_only {
                &variant.response[..variant.head_len]
            } else {
                &variant.response[..]
            };
            Served { bytes, close: true }
        }
        _ if head_only => keep(&cell.response[..cell.head_len]),
        _ => keep(&cell.response),
    }
}

/// The pre-built error response (with `connection: close`) for a refused request.
pub(crate) fn refusal(kind: Refusal) -> &'static [u8] {
    match kind {
        Refusal::BadRequest => &CATALOG.bad_request,
        Refusal::HeadTooLarge => &CATALOG.head_too_large,
        Refusal::VersionNotSupported => &CATALOG.version_not_supported,
    }
}

impl Catalog {
    fn build() -> Self {
        let gzip_len = PAYLOAD_GZIP.len().to_string();
        let big = repeat_to(PAYLOAD, ONE_MIB);
        let big_len = big.len().to_string();
        let gzip_head = head(
            "200 OK",
            &[
                ("content-type", JSON),
                ("content-encoding", "gzip"),
                ("content-length", &gzip_len),
                ("date", DATE),
            ],
        );
        let big_head = head(
            "200 OK",
            &[
                ("content-type", "application/octet-stream"),
                ("content-length", &big_len),
                ("date", DATE),
            ],
        );
        let cells = vec![
            Cell::new("/1k", HEAD_1K, PAYLOAD, PAYLOAD),
            Cell::new("/1k-h20", HEAD_1K_H20, PAYLOAD, PAYLOAD),
            Cell::new("/1k-gzip", &gzip_head, PAYLOAD_GZIP, PAYLOAD),
            Cell::chunked(
                "/1k-chunked",
                &[("content-type", JSON), ("date", DATE)],
                PAYLOAD,
            ),
            Cell::new("/1m", &big_head, &big, &big),
        ];
        Self {
            cells,
            not_found: head("404 Not Found", &[("content-length", "0"), ("date", DATE)]),
            bad_request: closing_head("400 Bad Request"),
            head_too_large: closing_head("431 Request Header Fields Too Large"),
            version_not_supported: closing_head("505 HTTP Version Not Supported"),
        }
    }
}

impl Cell {
    /// `wire_body` is the body as sent (framed, encoded); `body` is what a client decodes from it.
    fn new(path: &'static str, head: &[u8], wire_body: &[u8], body: &[u8]) -> Self {
        Self {
            path,
            response: [head, wire_body].concat(),
            head_len: head.len(),
            body_fnv: fnv1a64(body),
            close_delimited: None,
        }
    }

    /// A cell whose `body` goes out in chunked framing after `fields`, with a close-delimited variant for HTTP/1.0:
    /// the same `fields` without the framing field, so the two heads cannot drift apart.
    fn chunked(path: &'static str, fields: &[(&str, &str)], body: &[u8]) -> Self {
        let plain_head = head("200 OK", fields);
        let chunked_head = head(
            "200 OK",
            &[fields, &[("transfer-encoding", "chunked")]].concat(),
        );
        let variant = CloseDelimited {
            response: [&plain_head[..], body].concat(),
            head_len: plain_head.len(),
        };
        Self {
            close_delimited: Some(variant),
            ..Self::new(path, &chunked_head, &frame_chunked(body), body)
        }
    }
}

/// An HTTP/1.1 response head: status line, the fields in order, blank line.
fn head(status: &str, fields: &[(&str, &str)]) -> Vec<u8> {
    let mut head = format!("HTTP/1.1 {status}\r\n");
    for &(name, value) in fields {
        head.extend([name, ": ", value, "\r\n"]);
    }
    head.push_str("\r\n");
    head.into_bytes()
}

/// An empty error response that tells the client the connection closes after it.
fn closing_head(status: &str) -> Vec<u8> {
    head(
        status,
        &[
            ("content-length", "0"),
            ("connection", "close"),
            ("date", DATE),
        ],
    )
}

/// `body` in `CHUNK_BYTES` chunks plus the last chunk, without trailers.
fn frame_chunked(body: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(body.len() + 64);
    for chunk in body.chunks(CHUNK_BYTES) {
        framed.extend_from_slice(format!("{:x}\r\n", chunk.len()).as_bytes());
        framed.extend_from_slice(chunk);
        framed.extend_from_slice(b"\r\n");
    }
    framed.extend_from_slice(b"0\r\n\r\n");
    framed
}

fn repeat_to(pattern: &[u8], len: usize) -> Vec<u8> {
    pattern.iter().copied().cycle().take(len).collect()
}

/// FNV-1a, 64-bit (the per-request checksum every G0 client computes, design §16.2).
pub(crate) fn fnv1a64(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(PRIME)
    })
}

#[cfg(test)]
mod tests;
