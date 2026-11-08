//! blazehttp's `write_request` vs an emulation of hyper 1.12's client request encoder
//! (`src/proto/h1/role.rs` at v1.12.0: `Client::encode` L1215–1262, `set_content_length` L1540–1571,
//! `write_headers` L1629–1636), on the same request sets as `examples/h1_write_bench.rs`.
//!
//! - **hyper construct+encode (headline, equal work):** hyper takes a `Request` by value per call, built from the
//!   caller's target and fields. Timed per request: parse the target (`Uri::try_from`), clone the defaults-only map
//!   (host + client defaults: the template analogue), add each per-request field with `HeaderName::from_bytes` +
//!   `HeaderValue::from_bytes`, insert `content-length` (`HeaderValue::from(u64)`) and `connection: close` when due,
//!   then encode. Like our writer it validates the target and every per-request field on each call; it still applies
//!   less policy (no forbidden, singleton, line-count or authority checks).
//! - **hyper clone+encode (lower bound on hyper's cost):** clones a map whose values are already validated and never
//!   re-parses the target.
//! - **hyper encode-only (hyper's serialisation floor, no validation):** the map is pre-built with `content-length`.
//!   It is compared with our copy floor, never with our writer.
//!
//! Every variant is checked to write exactly our bytes before timing. Run: `cargo run --release --manifest-path
//! bench/writer/hyper-emul/Cargo.toml` (`-- --verify` checks only, as CI does). Prints one JSON line per run and a
//! Markdown table; results go to `docs/perf/journal.md`.

#[path = "../../../../crates/engine-core/tests/support/write_bench.rs"]
mod write_bench;
#[path = "../../../../crates/engine-core/tests/support/write_rings.rs"]
mod write_rings;

use std::fmt::Write as _;
use std::time::Instant;

use engine_core::h1::{HeaderTemplate, write_request};
use http::header::{CONNECTION, CONTENT_LENGTH, HeaderMap, HeaderName, HeaderValue};
use http::{Method, Uri};
use write_bench::{
    BLOCKS_PER_RUN, Framed, RUNS, WARM_UP, floor, frame, median, template_bytes, time_block,
};
use write_rings::{DEFAULT_LINES, DEFAULTS, HOST};

/// hyper's `AVERAGE_HEADER_SIZE` (role.rs L32).
const AVERAGE_HEADER_SIZE: usize = 30;
/// Variants in timing order: ours, our floor, hyper construct+encode, clone+encode, encode-only.
const VARIANTS: usize = 5;

/// One request in every shape the variants need.
struct Prepared {
    framed: Framed,
    method: Method,
    /// Per-request fields as the caller hands them to hyper: raw name and value bytes.
    delta: Vec<(Vec<u8>, Vec<u8>)>,
    /// host + client defaults (the template analogue).
    defaults: HeaderMap,
    /// host + defaults + per-request fields, validated, without the framing fields.
    headers: HeaderMap,
    /// The same plus the framing fields (the encode-only variant).
    framed_map: HeaderMap,
    uri: Uri,
}

/// `(name, value)` byte pairs of a `("name: value\r\n")*` block.
fn split_fields(block: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    block
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = line.strip_suffix(b"\r").expect("CRLF");
            let colon = line.iter().position(|&b| b == b':').expect("colon");
            (
                line[..colon].to_vec(),
                line[colon + 1..].trim_ascii().to_vec(),
            )
        })
        .collect()
}

fn map_of(pairs: &[(Vec<u8>, Vec<u8>)], mut map: HeaderMap) -> HeaderMap {
    for (name, value) in pairs {
        let name = HeaderName::from_bytes(name).expect("valid name");
        map.append(name, HeaderValue::from_bytes(value).expect("valid value"));
    }
    map
}

/// hyper's `set_content_length`, then the `connection: close` field our writer generates for the same request.
fn add_framing(headers: &mut HeaderMap, framed: &Framed) {
    if framed.content_length {
        let length = u64::try_from(framed.request.body.len()).expect("small");
        headers.insert(CONTENT_LENGTH, HeaderValue::from(length));
    }
    if framed.close {
        headers.insert(CONNECTION, HeaderValue::from_static("close"));
    }
}

fn prepare(framed: Framed) -> Prepared {
    let host = [&b"host: "[..], HOST, b"\r\n"].concat();
    let defaults = map_of(
        &split_fields(&[&host[..], DEFAULTS].concat()),
        HeaderMap::new(),
    );
    let delta = split_fields(&framed.request.headers);
    let headers = map_of(&delta, defaults.clone());
    let mut framed_map = headers.clone();
    add_framing(&mut framed_map, &framed);
    Prepared {
        method: Method::from_bytes(framed.request.method.as_str().as_bytes()).expect("method"),
        uri: Uri::try_from(framed.request.target.as_slice()).expect("origin-form uri"),
        delta,
        defaults,
        headers,
        framed_map,
        framed,
    }
}

/// hyper's `FastWrite`: `fmt::Write` straight into the buffer.
struct FastWrite<'a>(&'a mut Vec<u8>);

impl std::fmt::Write for FastWrite<'_> {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.0.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

/// hyper's `Client::encode` + `write_headers`, then the body into the same buffer (as our writer does).
fn encode(out: &mut Vec<u8>, p: &Prepared, uri: &Uri, headers: &HeaderMap) {
    out.clear();
    out.reserve(30 + headers.len() * AVERAGE_HEADER_SIZE);
    out.extend_from_slice(p.method.as_str().as_bytes());
    out.extend_from_slice(b" ");
    let _ = write!(FastWrite(out), "{uri} ");
    out.extend_from_slice(b"HTTP/1.1\r\n");
    for (name, value) in headers {
        out.extend_from_slice(name.as_str().as_bytes());
        out.extend_from_slice(b": ");
        out.extend_from_slice(value.as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&p.framed.request.body);
}

/// Equal work: target parsed and every per-request field validated on each call.
fn hyper_construct_encode(out: &mut Vec<u8>, p: &Prepared) {
    let uri = Uri::try_from(p.framed.request.target.as_slice()).expect("origin-form uri");
    let mut headers = map_of(&p.delta, p.defaults.clone());
    add_framing(&mut headers, &p.framed);
    encode(out, p, &uri, &headers);
}

fn hyper_clone_encode(out: &mut Vec<u8>, p: &Prepared) {
    let mut headers = p.headers.clone();
    add_framing(&mut headers, &p.framed);
    encode(out, p, &p.uri, &headers);
}

fn hyper_encode_only(out: &mut Vec<u8>, p: &Prepared) {
    encode(out, p, &p.uri, &p.framed_map);
}

/// Checks every hyper variant writes exactly our bytes.
fn verify(ring: &[Prepared], template: &HeaderTemplate) {
    let (mut ours, mut theirs) = (Vec::new(), Vec::new());
    for p in ring {
        let _ = write_request(&mut ours, &p.framed.request.parts(), template, HOST).expect("valid");
        for variant in [
            hyper_construct_encode,
            hyper_clone_encode,
            hyper_encode_only,
        ] {
            variant(&mut theirs, p);
            assert_eq!(theirs, ours, "a hyper variant writes our bytes");
        }
    }
}

/// One block of each variant, in `VARIANTS` order.
fn blocks(
    ring: &[Prepared],
    out: &mut Vec<u8>,
    template: &HeaderTemplate,
    template_bytes: &[u8],
) -> [f64; VARIANTS] {
    let ours = |out: &mut Vec<u8>, p: &Prepared| {
        let _ = write_request(out, &p.framed.request.parts(), template, HOST).expect("valid");
    };
    let copy = |out: &mut Vec<u8>, p: &Prepared| floor(out, &p.framed, template_bytes);
    [
        time_block(ring, out, ours),
        time_block(ring, out, copy),
        time_block(ring, out, hyper_construct_encode),
        time_block(ring, out, hyper_clone_encode),
        time_block(ring, out, hyper_encode_only),
    ]
}

/// Interleaved runs; returns the per-variant medians.
fn measure(name: &str, ring: &[Prepared], template: &HeaderTemplate) -> [f64; VARIANTS] {
    let template_bytes = template_bytes();
    let mut out = Vec::new();
    let start = Instant::now();
    while start.elapsed() < WARM_UP {
        let _ = blocks(ring, &mut out, template, &template_bytes);
    }
    let mut runs: [Vec<f64>; VARIANTS] = Default::default();
    for run in 0..RUNS {
        let mut ns = [0.0; VARIANTS];
        for _ in 0..BLOCKS_PER_RUN {
            for (sum, block) in ns
                .iter_mut()
                .zip(blocks(ring, &mut out, template, &template_bytes))
            {
                *sum += block;
            }
        }
        let [ours, copy, construct, cloned, encode] = ns.map(|n| n / f64::from(BLOCKS_PER_RUN));
        println!(
            "{{\"set\":\"{name}\",\"run\":{run},\"ours_ns\":{ours:.2},\"floor_ns\":{copy:.2},\
             \"hyper_construct_encode_ns\":{construct:.2},\"hyper_clone_encode_ns\":{cloned:.2},\"hyper_encode_ns\":{encode:.2}}}"
        );
        for (series, value) in runs.iter_mut().zip([ours, copy, construct, cloned, encode]) {
            series.push(value);
        }
    }
    runs.map(|mut series| median(&mut series))
}

fn main() {
    let template = HeaderTemplate::new(HOST, DEFAULTS, DEFAULT_LINES, &[]).expect("valid template");
    let sets = [
        ("get", write_rings::get()),
        ("post1k", write_rings::post1k()),
        ("varied", write_rings::varied()),
    ];
    let rings: Vec<(&str, Vec<Prepared>)> = sets
        .into_iter()
        .map(|(name, ring)| {
            (
                name,
                frame(ring, &template).0.into_iter().map(prepare).collect(),
            )
        })
        .collect();
    for (_, ring) in &rings {
        verify(ring, &template);
    }
    if write_bench::verify_only() {
        println!("verified: every hyper variant and the copy floor write the writer's bytes");
        return;
    }
    let rows: Vec<_> = rings
        .iter()
        .map(|(name, ring)| (name, measure(name, ring, &template)))
        .collect();
    println!();
    println!(
        "write_request vs hyper 1.12 encoder emulation; {RUNS} runs x {BLOCKS_PER_RUN} blocks, medians"
    );
    println!(
        "| set | ours ns | hyper construct+encode ns | ours / construct+encode | ours / clone+encode | our floor ns | \
         hyper encode-only ns | our floor / hyper encode-only |"
    );
    println!("|---|---|---|---|---|---|---|---|");
    for (name, [a, f, c, k, e]) in rows {
        println!(
            "| `{name}` | {a:.1} | {c:.1} | {:.3} | {:.3} | {f:.1} | {e:.1} | {:.3} |",
            a / c,
            a / k,
            f / e
        );
    }
}
