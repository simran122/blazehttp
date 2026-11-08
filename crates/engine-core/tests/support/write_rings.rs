//! The request sets of the writer benchmarks and the zero-allocation test (`docs/notes/request-writer.md` §8.1):
//! `get` (template-only), `post1k` (1 KiB JSON + 3 fields) and the `varied` ring (mixed methods, targets of 8–200
//! bytes, 0–6 extra fields of 4–80 value bytes, bodies of 0–2 KiB), generated deterministically. Shared by
//! `tests/h1_writer_alloc.rs`, `examples/h1_write_bench.rs` and `bench/writer/hyper-emul`.

use engine_core::generated::spec::Method;
use engine_core::h1::RequestParts;

pub const HOST: &[u8] = b"127.0.0.1:3999";
/// The client's default fields of every set's template.
pub const DEFAULTS: &[u8] =
    b"accept: application/json, text/plain, */*\r\nuser-agent: blazehttp/0.0.0\r\naccept-encoding: gzip, br\r\n";
const VARIANTS: usize = 64;
const VARIED: usize = 256;
const VARIED_METHODS: [Method; 7] = [
    Method::Get,
    Method::Post,
    Method::Put,
    Method::Delete,
    Method::Patch,
    Method::Options,
    Method::Query,
];

/// One owned request of a set.
pub struct Request {
    pub method: Method,
    pub target: Vec<u8>,
    /// `("name: value\r\n")*`.
    pub headers: Vec<u8>,
    /// The field count JS would pass, counted once here so timed loops do not re-count it.
    pub header_lines: u16,
    pub body: Vec<u8>,
}

impl Request {
    fn new(method: Method, target: Vec<u8>, headers: Vec<u8>, body: Vec<u8>) -> Self {
        let lines = headers.windows(2).filter(|w| w == b"\r\n").count();
        Self {
            method,
            target,
            headers,
            header_lines: u16::try_from(lines).expect("few lines"),
            body,
        }
    }

    /// The writer's view.
    pub fn parts(&self) -> RequestParts<'_> {
        RequestParts {
            method: self.method,
            target: &self.target,
            headers: &self.headers,
            header_lines: self.header_lines,
            body: &self.body,
        }
    }
}

/// xorshift64: a fixed, seedable sequence (no dependency).
fn next(state: &mut u64, bound: usize) -> usize {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    usize::try_from(*state % u64::try_from(bound).expect("small")).expect("small")
}

/// `GET /data/NN`: template-only requests (no per-request field, no body).
pub fn get() -> Vec<Request> {
    (0..VARIANTS)
        .map(|i| {
            Request::new(
                Method::Get,
                format!("/data/{i:02}").into_bytes(),
                Vec::new(),
                Vec::new(),
            )
        })
        .collect()
}

/// `POST /api/items` with a 1 KiB JSON body and `content-type`, `x-request-id`, `authorization`.
pub fn post1k() -> Vec<Request> {
    (0..VARIANTS)
        .map(|i| {
            let headers = format!(
                "content-type: application/json\r\nx-request-id: 7f3c9a2e-{i:04}-4b8d-9e1f-2a6c8d0b4e{i:02}\r\n\
                 authorization: Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiJiZW5jaCJ9.c2lnbmF0dXJl{i:02}\r\n"
            );
            let item = format!("{{\"id\":{i},\"name\":\"item-{i}\",\"tags\":[\"a\",\"b\"],\"v\":1.5}},");
            let mut body = format!("{{\"items\":[{}", item.repeat(1024 / item.len())).into_bytes();
            body.resize(1023, b' ');
            body.push(b']');
            Request::new(Method::Post, b"/api/items".to_vec(), headers.into_bytes(), body)
        })
        .collect()
}

/// The varied ring of 256 requests.
pub fn varied() -> Vec<Request> {
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..VARIED)
        .map(|i| {
            let fields = next(&mut state, 7);
            let headers: Vec<u8> = (0..fields)
                .flat_map(|f| {
                    format!("x-field-{f}: {}\r\n", "v".repeat(4 + next(&mut state, 77)))
                        .into_bytes()
                })
                .collect();
            let target = [&b"/"[..], &vec![b'p'; 7 + next(&mut state, 193)]].concat();
            let body = vec![b'b'; next(&mut state, 2049)];
            Request::new(
                VARIED_METHODS[i % VARIED_METHODS.len()],
                target,
                headers,
                body,
            )
        })
        .collect()
}
