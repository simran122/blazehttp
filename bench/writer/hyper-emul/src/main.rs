//! blazehttp's `write_request` vs an emulation of hyper 1.12's client request encoder
//! (`src/proto/h1/role.rs` at v1.12.0: `Client::encode` L1215–1262, `set_content_length` L1540–1571,
//! `write_headers` L1629–1636), on the same request sets as `examples/h1_write_bench.rs`.
//!
//! - **hyper (clone + encode):** hyper takes a `Request` by value per call, so every request carries its own
//!   `HeaderMap` (host, defaults and per-request fields): the emulation clones a pre-built map, inserts
//!   `content-length` as `HeaderValue::from(u64)` when the length is due, reserves `30 + 30 × fields`, writes method,
//!   SP, the URI through `Display` (hyper's `TODO: … encode without std::fmt`), ` HTTP/1.1`, CRLF, every field and the
//!   blank line, then appends the body to the same buffer (as our writer does; hyper itself queues it for `writev`).
//! - **hyper (encode only):** the same serialisation from a map that already holds `content-length` (no clone).
//!
//! Run: `cargo run --release --manifest-path bench/writer/hyper-emul/Cargo.toml`. Prints one JSON line per run and a
//! Markdown table; results go to `docs/perf/journal.md`.

#[path = "../../../../crates/engine-core/tests/support/write_rings.rs"]
mod write_rings;

use std::fmt::Write as _;
use std::hint::black_box;
use std::time::{Duration, Instant};

use engine_core::h1::{HeaderTemplate, write_request};
use http::header::{CONNECTION, CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Uri};
use write_rings::{DEFAULTS, HOST, Request};

const RUNS: usize = 7;
const BLOCKS_PER_RUN: u32 = 4;
const ITERATIONS_PER_BLOCK: usize = 100_000;
const WARM_UP: Duration = Duration::from_millis(100);
/// hyper's `AVERAGE_HEADER_SIZE` (role.rs L32).
const AVERAGE_HEADER_SIZE: usize = 30;

/// One request in both shapes: blazehttp's parts and hyper's typed head.
struct Prepared {
    ours: Request,
    method: Method,
    uri: Uri,
    /// host + defaults + per-request fields, without `content-length`.
    headers: HeaderMap,
    /// The same plus the generated `content-length` when due (the encode-only variant).
    framed: HeaderMap,
    length: Option<u64>,
    /// Whether our writer announced `connection: close` (a body on `GET`/`DELETE`/`OPTIONS`); the emulation sends
    /// the same field so both write identical bytes.
    close: bool,
}

/// `(name, value)` pairs of a `("name: value\r\n")*` block.
fn fields(block: &[u8]) -> Vec<(HeaderName, HeaderValue)> {
    block
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = line.strip_suffix(b"\r").expect("CRLF");
            let colon = line.iter().position(|&b| b == b':').expect("colon");
            let name = HeaderName::from_bytes(&line[..colon]).expect("valid name");
            let value =
                HeaderValue::from_bytes(line[colon + 1..].trim_ascii()).expect("valid value");
            (name, value)
        })
        .collect()
}

fn prepare(ours: Request, out: &mut Vec<u8>, template: &HeaderTemplate) -> Prepared {
    let _ = write_request(out, &ours.parts(), template, HOST).expect("valid bench request");
    let length = out
        .windows(18)
        .any(|w| w == b"\r\ncontent-length: ")
        .then(|| u64::try_from(ours.body.len()).expect("small"));
    let host = [&b"host: "[..], HOST, b"\r\n"].concat();
    let mut headers = HeaderMap::new();
    for (name, value) in fields(&[&host[..], DEFAULTS, &ours.headers].concat()) {
        headers.append(name, value);
    }
    let close = out.windows(21).any(|w| w == b"\r\nconnection: close\r\n");
    let mut framed = headers.clone();
    add_framing(&mut framed, length, close);
    let prepared = Prepared {
        method: Method::from_bytes(ours.method.as_str().as_bytes()).expect("method"),
        uri: Uri::try_from(ours.target.as_slice()).expect("origin-form uri"),
        headers,
        framed,
        length,
        close,
        ours,
    };
    let mut theirs = Vec::new();
    hyper_clone_encode(&mut theirs, &prepared);
    assert_eq!(theirs, *out, "both encoders write the same request");
    prepared
}

/// hyper's `FastWrite`: `fmt::Write` straight into the buffer.
struct FastWrite<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for FastWrite<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn encode(out: &mut Vec<u8>, p: &Prepared, headers: &HeaderMap) {
    out.clear();
    out.reserve(30 + headers.len() * AVERAGE_HEADER_SIZE);
    out.extend_from_slice(p.method.as_str().as_bytes());
    out.extend_from_slice(b" ");
    let _ = write!(FastWrite(out), "{} ", p.uri);
    out.extend_from_slice(b"HTTP/1.1\r\n");
    for (name, value) in headers {
        out.extend_from_slice(name.as_str().as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&p.ours.body);
}

/// hyper's `set_content_length` (`HeaderValue::from(u64)`), then the caller's `connection: close` field.
fn add_framing(headers: &mut HeaderMap, length: Option<u64>, close: bool) {
    if let Some(length) = length {
        headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    }
    if close {
        headers.insert(CONNECTION, HeaderValue::from_static("close"));
    }
}

fn hyper_clone_encode(out: &mut Vec<u8>, p: &Prepared) {
    let mut headers = p.headers.clone();
    add_framing(&mut headers, p.length, p.close);
    encode(out, p, &headers);
}

fn time_block(
    ring: &[Prepared],
    out: &mut Vec<u8>,
    write: impl Fn(&mut Vec<u8>, &Prepared),
) -> f64 {
    let start = Instant::now();
    for i in 0..ITERATIONS_PER_BLOCK {
        write(black_box(&mut *out), black_box(&ring[i % ring.len()]));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations and counts far below 2^52"
    )]
    let ns = start.elapsed().as_nanos() as f64 / ITERATIONS_PER_BLOCK as f64;
    ns
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Interleaved runs; returns medians of ours, hyper clone + encode, hyper encode-only and of the two ratios.
fn measure(name: &str, ring: &[Prepared], template: &HeaderTemplate) -> [f64; 5] {
    let mut out = Vec::new();
    let ours = |out: &mut Vec<u8>, p: &Prepared| {
        let _ = write_request(out, &p.ours.parts(), template, HOST).expect("valid");
    };
    let only = |out: &mut Vec<u8>, p: &Prepared| encode(out, p, &p.framed);
    let start = Instant::now();
    while start.elapsed() < WARM_UP {
        time_block(ring, &mut out, ours);
        time_block(ring, &mut out, hyper_clone_encode);
        time_block(ring, &mut out, only);
    }
    let mut runs: [Vec<f64>; 5] = Default::default();
    for run in 0..RUNS {
        let mut ns = [0.0; 3];
        for _ in 0..BLOCKS_PER_RUN {
            ns[0] += time_block(ring, &mut out, ours);
            ns[1] += time_block(ring, &mut out, hyper_clone_encode);
            ns[2] += time_block(ring, &mut out, only);
        }
        let [a, b, c] = ns.map(|n| n / f64::from(BLOCKS_PER_RUN));
        println!(
            "{{\"set\":\"{name}\",\"run\":{run},\"ours_ns\":{a:.2},\"hyper_clone_encode_ns\":{b:.2},\"hyper_encode_ns\":{c:.2}}}"
        );
        for (series, value) in runs.iter_mut().zip([a, b, c, a / b, a / c]) {
            series.push(value);
        }
    }
    runs.map(|mut series| median(&mut series))
}

fn main() {
    let template = HeaderTemplate::new(HOST, DEFAULTS, &[]).expect("valid template");
    let mut out = Vec::new();
    let sets = [
        ("get", write_rings::get()),
        ("post1k", write_rings::post1k()),
        ("varied", write_rings::varied()),
    ];
    let mut rows = Vec::new();
    for (name, ring) in sets {
        let ring: Vec<Prepared> = ring
            .into_iter()
            .map(|r| prepare(r, &mut out, &template))
            .collect();
        rows.push((name, measure(name, &ring, &template)));
    }
    println!();
    println!(
        "write_request vs hyper 1.12 encoder emulation; {RUNS} runs x {BLOCKS_PER_RUN} blocks, medians"
    );
    println!(
        "| set | ours ns | hyper clone+encode ns | hyper encode-only ns | ours / clone+encode | ours / encode-only |"
    );
    println!("|---|---|---|---|---|---|");
    for (name, [a, b, c, r1, r2]) in rows {
        println!("| `{name}` | {a:.1} | {b:.1} | {c:.1} | {r1:.3} | {r2:.3} |");
    }
}
