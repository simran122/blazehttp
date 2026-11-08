//! The benchmark cells: every response pre-built once, at start-up (design note §8).
//!
//! **Singleton** (design §13): `CATALOG` is a process-wide `LazyLock`, so the cells are built once per process and
//! every connection task shares the same immutable bytes.

use std::sync::LazyLock;

/// Heads shared with the parser tests and benchmark, so the parser is measured on the bytes the server sends.
const HEAD_1K: &[u8] = include_bytes!("../../../tests/fixtures/h1-heads/1k.http");
const HEAD_1K_H20: &[u8] = include_bytes!("../../../tests/fixtures/h1-heads/1k-h20.http");
/// The 847-byte JSON payload of every 1 KiB cell.
const PAYLOAD: &[u8] = include_bytes!("../../../tests/fixtures/bodies/1k.json");
/// `PAYLOAD` gzipped once and committed, so the served bytes never depend on a compressor version.
const PAYLOAD_GZIP: &[u8] = include_bytes!("../../../tests/fixtures/bodies/1k.json.gz");

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

/// Why a request is answered with an error and the connection closed.
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
        let chunked_head = head(
            "200 OK",
            &[
                ("content-type", JSON),
                ("transfer-encoding", "chunked"),
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
                &chunked_head,
                &head("200 OK", &[("content-type", JSON), ("date", DATE)]),
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

    /// A cell whose `body` goes out in chunked framing, with a close-delimited variant (`plain_head`) for HTTP/1.0.
    fn chunked(path: &'static str, head: &[u8], plain_head: &[u8], body: &[u8]) -> Self {
        let variant = CloseDelimited {
            response: [plain_head, body].concat(),
            head_len: plain_head.len(),
        };
        Self {
            close_delimited: Some(variant),
            ..Self::new(path, head, &frame_chunked(body), body)
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
mod tests {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::LazyLock;
    use std::time::{Duration, Instant};

    use flate2::read::GzDecoder;

    use super::{
        CATALOG, DATE, HEAD_1K, JSON, PAYLOAD, PAYLOAD_GZIP, Refusal, Served, cells, fnv1a64, head,
        prebuild, refusal, response_for,
    };
    use crate::request::find_head_end;

    fn split(response: &[u8]) -> (&[u8], &[u8]) {
        let end = find_head_end(response, 0).expect("blank line") + 4;
        response.split_at(end)
    }

    fn field<'a>(head: &'a [u8], name: &str) -> Option<&'a str> {
        let head = std::str::from_utf8(head).expect("ASCII head");
        head.split("\r\n")
            .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
    }

    fn gunzip(body: &[u8]) -> Vec<u8> {
        let mut decoded = Vec::new();
        GzDecoder::new(body)
            .read_to_end(&mut decoded)
            .expect("gzip");
        decoded
    }

    fn dechunk(mut framed: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        loop {
            let line_end = framed
                .windows(2)
                .position(|w| w == b"\r\n")
                .expect("chunk size line");
            let size =
                usize::from_str_radix(std::str::from_utf8(&framed[..line_end]).expect("hex"), 16)
                    .expect("size");
            if size == 0 {
                assert_eq!(&framed[line_end..], b"\r\n\r\n", "last chunk, no trailers");
                return body;
            }
            let data = &framed[line_end + 2..];
            body.extend_from_slice(&data[..size]);
            assert_eq!(&data[size..size + 2], b"\r\n");
            framed = &data[size + 2..];
        }
    }

    fn decoded_body(head: &[u8], body: &[u8]) -> Vec<u8> {
        if field(head, "transfer-encoding") == Some("chunked") {
            return dechunk(body);
        }
        let len: usize = field(head, "content-length")
            .expect("framing")
            .parse()
            .expect("length");
        assert_eq!(len, body.len(), "content-length matches the body");
        if field(head, "content-encoding") == Some("gzip") {
            gunzip(body)
        } else {
            body.to_vec()
        }
    }

    #[test]
    fn head_builder_reproduces_the_1k_fixture() {
        let len = PAYLOAD.len().to_string();
        let built = head(
            "200 OK",
            &[
                ("content-type", JSON),
                ("content-length", &len),
                ("date", DATE),
            ],
        );
        assert_eq!(built, HEAD_1K);
    }

    #[test]
    fn every_cell_is_framed_correctly_and_matches_its_checksum() {
        for cell in cells() {
            let (head, body) = split(&cell.response);
            let decoded = decoded_body(head, body);
            assert_eq!(fnv1a64(&decoded), cell.body_fnv, "{}", cell.path);
            if cell.path != "/1m" {
                assert_eq!(decoded, PAYLOAD, "{}", cell.path);
            }
        }
    }

    #[test]
    fn committed_gzip_decodes_to_the_payload() {
        assert_eq!(gunzip(PAYLOAD_GZIP), PAYLOAD);
    }

    #[test]
    fn head_only_responses_end_at_the_blank_line() {
        for cell in cells() {
            let head = response_for(cell.path.as_bytes(), true, false).bytes;
            assert_eq!(head, split(&cell.response).0, "{}", cell.path);
            assert_eq!(cell.head_len, head.len(), "{}", cell.path);
        }
        assert_eq!(
            response_for(b"/nope", true, false),
            response_for(b"/nope", false, false)
        );
    }

    #[test]
    fn http10_gets_chunked_cells_close_delimited() {
        for cell in cells() {
            let answer = response_for(cell.path.as_bytes(), false, true);
            let Some(variant) = &cell.close_delimited else {
                assert_eq!(
                    answer,
                    Served {
                        bytes: &cell.response,
                        close: false
                    },
                    "{}",
                    cell.path
                );
                continue;
            };
            assert_eq!(
                answer,
                Served {
                    bytes: &variant.response,
                    close: true
                },
                "{}",
                cell.path
            );
            let head = &variant.response[..variant.head_len];
            assert!(
                !String::from_utf8_lossy(head).contains("transfer-encoding"),
                "{}",
                cell.path
            );
            assert_eq!(
                &variant.response[variant.head_len..],
                PAYLOAD,
                "{}",
                cell.path
            );
            let head_only = response_for(cell.path.as_bytes(), true, true);
            assert_eq!(
                head_only,
                Served {
                    bytes: head,
                    close: true
                },
                "{}",
                cell.path
            );
        }
        assert!(
            cells().iter().any(|cell| cell.close_delimited.is_some()),
            "the chunked cell has a variant"
        );
    }

    #[test]
    fn refusals_close_the_connection() {
        for (kind, status) in [
            (Refusal::BadRequest, "400 "),
            (Refusal::VersionNotSupported, "505 "),
            (Refusal::HeadTooLarge, "431 "),
        ] {
            let (head, body) = split(refusal(kind));
            assert!(
                head.starts_with(format!("HTTP/1.1 {status}").as_bytes()),
                "{kind:?}"
            );
            assert_eq!(field(head, "connection"), Some("close"), "{kind:?}");
            assert!(body.is_empty(), "{kind:?}");
        }
    }

    /// Set by [`prebuild_builds_the_catalog_in_a_fresh_process`] for the child it starts.
    const FRESH_PROCESS: &str = "BENCH_SERVER_FRESH_PROCESS";

    /// Must run alone in a fresh process (other tests build the catalog as a side effect), so it is ignored here and
    /// run by [`prebuild_builds_the_catalog_in_a_fresh_process`]. Run any other way (`--include-ignored` in the
    /// shared process), it checks nothing instead of failing at random.
    #[test]
    #[ignore = "run in a fresh process by prebuild_builds_the_catalog_in_a_fresh_process"]
    fn prebuild_builds_the_catalog() {
        if std::env::var_os(FRESH_PROCESS).is_none() {
            return;
        }
        assert!(
            LazyLock::get(&CATALOG).is_none(),
            "nothing built the catalog yet"
        );
        prebuild();
        assert!(
            LazyLock::get(&CATALOG).is_some(),
            "prebuild built the catalog"
        );
    }

    /// How long the child test process may take.
    const CHILD_WITHIN: Duration = Duration::from_secs(30);

    #[test]
    fn prebuild_builds_the_catalog_in_a_fresh_process() {
        let exe = std::env::current_exe().expect("test binary");
        let name = "cells::tests::prebuild_builds_the_catalog";
        let mut child = Command::new(exe)
            .args(["--exact", name, "--ignored", "--test-threads", "1"])
            .env(FRESH_PROCESS, "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("start the test binary");
        let started = Instant::now();
        while child.try_wait().expect("child status").is_none() {
            if started.elapsed() > CHILD_WITHIN {
                let _ = child.kill();
                panic!("the fresh-process test ran past {CHILD_WITHIN:?}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let run = child.wait_with_output().expect("child output");
        let stdout = String::from_utf8_lossy(&run.stdout);
        assert!(
            run.status.success(),
            "{stdout}{}",
            String::from_utf8_lossy(&run.stderr)
        );
        assert!(
            stdout.contains("1 passed"),
            "the ignored test ran: {stdout}"
        );
    }

    #[test]
    fn fnv1a64_matches_the_reference_vectors() {
        // Test vectors from the FNV reference (draft-eastlake-fnv, "" and "a").
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
