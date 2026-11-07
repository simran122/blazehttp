//! The benchmark cells: every response pre-built once, at start-up (design note §8).

use std::io::Write;
use std::sync::LazyLock;

use flate2::Compression;
use flate2::write::GzEncoder;

/// Heads shared with the parser tests and benchmark, so the parser is measured on the bytes the server sends.
const HEAD_1K: &[u8] = include_bytes!("../../../tests/fixtures/h1-heads/1k.http");
const HEAD_1K_H20: &[u8] = include_bytes!("../../../tests/fixtures/h1-heads/1k-h20.http");
/// The 847-byte JSON payload of every 1 KiB cell.
const PAYLOAD: &[u8] = include_bytes!("../../../tests/fixtures/bodies/1k.json");

const DATE: &str = "Fri, 06 Nov 2026 12:00:00 GMT";
const JSON: &str = "application/json";
const CHUNK_BYTES: usize = 256;
const ONE_MIB: usize = 1 << 20;

/// One benchmark cell: a request path and its complete pre-built response.
#[derive(Debug)]
pub struct Cell {
    pub path: &'static str,
    /// Status line, head and framed body, exactly as written to the socket.
    pub response: Vec<u8>,
    /// FNV-1a 64 of the decoded body (no chunked framing, no gzip): the bytes a client's application receives.
    pub body_fnv: u64,
}

struct Catalog {
    cells: Vec<Cell>,
    not_found: Vec<u8>,
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

/// The pre-built response for a request target; unknown targets get a pre-built 404.
pub(crate) fn response_for(target: &[u8]) -> &'static [u8] {
    let cell = CATALOG
        .cells
        .iter()
        .find(|cell| cell.path.as_bytes() == target);
    cell.map_or(&CATALOG.not_found, |cell| &cell.response)
}

impl Catalog {
    fn build() -> Self {
        let gzipped = gzip(PAYLOAD);
        let gzip_len = gzipped.len().to_string();
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
        let big = repeat_to(PAYLOAD, ONE_MIB);
        let big_len = big.len().to_string();
        let big_head = head(
            "200 OK",
            &[
                ("content-type", "application/octet-stream"),
                ("content-length", &big_len),
                ("date", DATE),
            ],
        );
        let cells = vec![
            Cell::new("/1k", &[HEAD_1K, PAYLOAD], PAYLOAD),
            Cell::new("/1k-h20", &[HEAD_1K_H20, PAYLOAD], PAYLOAD),
            Cell::new("/1k-gzip", &[&gzip_head, &gzipped], PAYLOAD),
            Cell::new("/1k-chunked", &[&chunked_head, &chunked(PAYLOAD)], PAYLOAD),
            Cell::new("/1m", &[&big_head, &big], &big),
        ];
        let not_found = head("404 Not Found", &[("content-length", "0"), ("date", DATE)]);
        Self { cells, not_found }
    }
}

impl Cell {
    fn new(path: &'static str, parts: &[&[u8]], body: &[u8]) -> Self {
        Self {
            path,
            response: parts.concat(),
            body_fnv: fnv1a64(body),
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

fn gzip(body: &[u8]) -> Vec<u8> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder
        .write_all(body)
        .expect("writing into a Vec cannot fail");
    encoder.finish().expect("writing into a Vec cannot fail")
}

/// `body` in `CHUNK_BYTES` chunks plus the last chunk, without trailers.
fn chunked(body: &[u8]) -> Vec<u8> {
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

    use flate2::read::GzDecoder;

    use super::{DATE, HEAD_1K, JSON, PAYLOAD, cells, fnv1a64, head};
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
        if field(head, "content-encoding") != Some("gzip") {
            return body.to_vec();
        }
        let mut decoded = Vec::new();
        GzDecoder::new(body)
            .read_to_end(&mut decoded)
            .expect("gzip");
        decoded
    }

    #[test]
    fn head_builder_reproduces_the_1k_fixture() {
        let built = head(
            "200 OK",
            &[
                ("content-type", JSON),
                ("content-length", "847"),
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
    fn fnv1a64_matches_the_reference_vectors() {
        // Test vectors from the FNV reference (draft-eastlake-fnv, "" and "a").
        assert_eq!(fnv1a64(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a64(b"a"), 0xaf63_dc4c_8601_ec8c);
    }
}
