//! Shared pieces of the request-writer benchmarks (`examples/h1_write_bench.rs`, `bench/writer/hyper-emul`): each ring
//! request with the framing the writer chose for it, the copy floor (note §8.1), the timing loop and the `--verify`
//! switch that CI uses to run every byte-equality check once without timing.

use std::fmt::NumBuffer;
use std::hint::black_box;
use std::time::{Duration, Instant};

use engine_core::h1::{HeaderTemplate, Reuse, write_request};

use super::write_rings::{DEFAULTS, HOST, Request};

pub const RUNS: usize = 7;
pub const BLOCKS_PER_RUN: u32 = 4;
const ITERATIONS_PER_BLOCK: usize = 100_000;
pub const WARM_UP: Duration = Duration::from_millis(100);

/// Whether the binary runs in verify-only mode (`--verify`): checks, no timing.
#[must_use]
pub fn verify_only() -> bool {
    std::env::args().any(|arg| arg == "--verify")
}

/// The template's bytes as the floor appends them: `host:` line + defaults.
#[must_use]
pub fn template_bytes() -> Vec<u8> {
    [&b"host: "[..], HOST, b"\r\n", DEFAULTS].concat()
}

/// A ring request with the framing lines the writer chose for it.
pub struct Framed {
    pub request: Request,
    pub content_length: bool,
    pub close: bool,
}

/// Writes each request once (untimed), takes its framing from the writer — `close` from the returned [`Reuse`],
/// `content-length` from the written head — and checks that the floor writes the same bytes. Returns the framed ring
/// and its mean head length.
///
/// # Panics
///
/// When a ring request is invalid or the floor's bytes differ from the writer's.
#[must_use]
pub fn frame(ring: Vec<Request>, template: &HeaderTemplate) -> (Vec<Framed>, usize) {
    let template_bytes = template_bytes();
    let (mut out, mut copy, mut head) = (Vec::new(), Vec::new(), 0);
    let framed: Vec<Framed> = ring
        .into_iter()
        .map(|request| {
            let reuse = write_request(&mut out, &request.parts(), template, HOST)
                .expect("valid bench request");
            let head_len = out.len() - request.body.len();
            let framed = Framed {
                content_length: out[..head_len]
                    .windows(18)
                    .any(|w| w == b"\r\ncontent-length: "),
                close: reuse == Reuse::CloseAfterExchange,
                request,
            };
            floor(&mut copy, &framed, &template_bytes);
            assert_eq!(copy, out, "the floor writes the writer's bytes");
            head += head_len;
            framed
        })
        .collect();
    let mean = head / framed.len();
    (framed, mean)
}

/// The copy floor (note §8.1): the writer's bytes, appended in the same order, without any check.
pub fn floor(out: &mut Vec<u8>, framed: &Framed, template_bytes: &[u8]) {
    let request = &framed.request;
    out.clear();
    out.extend_from_slice(request.method.as_str().as_bytes());
    out.extend_from_slice(b" ");
    out.extend_from_slice(&request.target);
    out.extend_from_slice(b" HTTP/1.1\r\n");
    out.extend_from_slice(template_bytes);
    out.extend_from_slice(&request.headers);
    if framed.content_length {
        let mut digits = NumBuffer::new();
        out.extend_from_slice(b"content-length: ");
        out.extend_from_slice(request.body.len().format_into(&mut digits).as_bytes());
        out.extend_from_slice(b"\r\n");
    }
    if framed.close {
        out.extend_from_slice(b"connection: close\r\n");
    }
    out.extend_from_slice(b"\r\n");
    out.extend_from_slice(&request.body);
}

/// Nanoseconds per request over a block of requests cycling through `ring`.
pub fn time_block<T>(ring: &[T], out: &mut Vec<u8>, write: impl Fn(&mut Vec<u8>, &T)) -> f64 {
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

/// The median of `values` (sorted in place).
pub fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
