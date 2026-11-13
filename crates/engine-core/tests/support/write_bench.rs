//! Shared pieces of the request-writer benchmarks (`examples/h1_write_bench.rs`, `bench/writer/hyper-emul`): each ring
//! request with the framing the writer chose for it, the baselines of note §8.1, the rotated timing loop and the
//! `--verify` switch that CI uses to run every byte-equality check once without timing.
//!
//! Baselines, cheapest first, each checked to write the writer's exact bytes:
//! - [`memcpy`]: one copy of the finished request — the lower bound for any writer that fills a buffer;
//! - [`unchecked_appends`] (called the "copy floor" in records before 2026-11-13): the same bytes as 7–11
//!   `extend_from_slice` calls behind two framing branches — no validation, no length sum, no reservation;
//! - [`writer_shape`]: `write_request`'s own append shape (12 pieces, checked length sum, one exact reservation)
//!   without any check, so `writer − shape` is the cost of the checks and `shape − appends` that of the shape.

use std::fmt::{NumBuffer, Write as _};
use std::hint::black_box;
use std::time::{Duration, Instant};

use engine_core::h1::{HeaderTemplate, Reuse, write_request};

use super::write_rings::{DEFAULTS, HOST, Request};

pub const RUNS: usize = 7;
pub const BLOCKS_PER_RUN: u32 = 4;
const ITERATIONS_PER_BLOCK: usize = 100_000;
const WARM_UP: Duration = Duration::from_millis(100);

/// Whether the binary runs in verify-only mode (`--verify`): checks, no timing.
#[must_use]
pub fn verify_only() -> bool {
    std::env::args().any(|arg| arg == "--verify")
}

/// The template's bytes as the baselines append them: `host:` line + defaults.
#[must_use]
pub fn template_bytes() -> Vec<u8> {
    [&b"host: "[..], HOST, b"\r\n", DEFAULTS].concat()
}

/// A ring request with the framing lines the writer chose for it and the bytes it wrote.
pub struct Framed {
    pub request: Request,
    pub content_length: bool,
    pub close: bool,
    /// The whole request as `write_request` wrote it, for [`memcpy`].
    pub wire: Vec<u8>,
}

/// Writes each request once (untimed), takes its framing from the writer — `close` from the returned [`Reuse`],
/// `content-length` from the written head — and checks that every baseline writes the same bytes. Returns the framed
/// ring and its mean head length.
///
/// # Panics
///
/// When a ring request is invalid or a baseline's bytes differ from the writer's.
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
                wire: out.clone(),
                request,
            };
            memcpy(&mut copy, &framed);
            assert_eq!(copy, out, "memcpy writes the writer's bytes");
            unchecked_appends(&mut copy, &framed, &template_bytes);
            assert_eq!(copy, out, "the unchecked appends write the writer's bytes");
            writer_shape(&mut copy, &framed, &template_bytes);
            assert_eq!(copy, out, "the writer shape writes the writer's bytes");
            head += head_len;
            framed
        })
        .collect();
    let mean = head / framed.len();
    (framed, mean)
}

/// One copy of the finished request.
pub fn memcpy(out: &mut Vec<u8>, framed: &Framed) {
    out.clear();
    out.extend_from_slice(&framed.wire);
}

/// The writer's bytes, appended piece by piece in wire order, without any check.
pub fn unchecked_appends(out: &mut Vec<u8>, framed: &Framed, template_bytes: &[u8]) {
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

/// `write_request`'s append shape with its checks removed: the same 12-piece list as `request_pieces`, the checked
/// length sum and the one exact reservation (`src/h1/writer/mod.rs`; keep in step, `frame` checks the bytes).
///
/// # Panics
///
/// When the length overflows or the reservation fails; neither happens for a bench request.
pub fn writer_shape(out: &mut Vec<u8>, framed: &Framed, template_bytes: &[u8]) {
    let request = &framed.request;
    out.clear();
    let mut digits = NumBuffer::new();
    let (length_prefix, length, length_end): (&[u8], &[u8], &[u8]) = if framed.content_length {
        let length = request.body.len().format_into(&mut digits).as_bytes();
        (b"content-length: ", length, b"\r\n")
    } else {
        (&[], &[], &[])
    };
    let close: &[u8] = if framed.close {
        b"connection: close\r\n"
    } else {
        &[]
    };
    let pieces: [&[u8]; 12] = [
        request.method.as_str().as_bytes(),
        b" ",
        &request.target,
        b" HTTP/1.1\r\n",
        template_bytes,
        &request.headers,
        length_prefix,
        length,
        length_end,
        close,
        b"\r\n",
        &request.body,
    ];
    let total = pieces
        .iter()
        .try_fold(0_usize, |sum, piece| sum.checked_add(piece.len()))
        .expect("bench requests are small");
    out.try_reserve_exact(total)
        .expect("bench requests are small");
    for piece in pieces {
        out.extend_from_slice(piece);
    }
}

/// Nanoseconds per request over a block of requests cycling through `ring`.
pub fn time_block<T>(
    ring: &[T],
    out: &mut Vec<u8>,
    mut write: impl FnMut(&mut Vec<u8>, &T),
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

/// One block of each of `N` variants, starting with variant `round % N` and wrapping, so drift within a run does not
/// always land on the same variant. `time(v)` times variant `v`; the result is indexed by variant.
fn rotated_blocks<const N: usize>(round: usize, mut time: impl FnMut(usize) -> f64) -> [f64; N] {
    let mut ns = [0.0; N];
    for k in 0..N {
        let variant = (round + k) % N;
        ns[variant] = time(variant);
    }
    ns
}

/// A warm-up, then [`RUNS`] runs of [`BLOCKS_PER_RUN`] rotated rounds of all variants. Prints one JSON line per run
/// (`set`, `run` and `<name>_ns` per variant) and returns each variant's per-run means, indexed like `names`.
pub fn measure_variants<const N: usize>(
    set: &str,
    names: [&str; N],
    mut time: impl FnMut(usize) -> f64,
) -> [Vec<f64>; N] {
    let start = Instant::now();
    let mut round = 0;
    while start.elapsed() < WARM_UP {
        let _ = rotated_blocks::<N>(round, &mut time);
        round += 1;
    }
    let mut runs: [Vec<f64>; N] = std::array::from_fn(|_| Vec::with_capacity(RUNS));
    for run in 0..RUNS {
        let mut sums = [0.0; N];
        for _ in 0..BLOCKS_PER_RUN {
            for (sum, ns) in sums.iter_mut().zip(rotated_blocks::<N>(round, &mut time)) {
                *sum += ns;
            }
            round += 1;
        }
        let means = sums.map(|sum| sum / f64::from(BLOCKS_PER_RUN));
        let mut line = format!("{{\"set\":\"{set}\",\"run\":{run}");
        for (name, ns) in names.iter().zip(means) {
            let _ = write!(line, ",\"{name}_ns\":{ns:.2}");
        }
        println!("{line}}}");
        for (series, ns) in runs.iter_mut().zip(means) {
            series.push(ns);
        }
    }
    runs
}

/// The median of `values` (sorted in place).
pub fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}
