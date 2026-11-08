//! Development benchmark: `write_request` vs the copy floor (`docs/notes/request-writer.md` §8.1, §8.4). Run with
//! `cargo run --release -p engine-core --example h1_write_bench`. The copy floor appends the same bytes with the same
//! `extend_from_slice` sequence and the same framing result (taken from the writer once, untimed) but no validation,
//! no limit, authority, singleton or line-count check, no checked length sum and no reservation, so
//! writer − floor = validation + bookkeeping. Prints one JSON line per run and a Markdown table. Results are dev
//! measurements (`docs/perf/journal.md`); P1a replaces this with a criterion bench.

#[path = "../tests/support/write_rings.rs"]
mod write_rings;

use std::fmt::NumBuffer;
use std::hint::black_box;
use std::time::{Duration, Instant};

use engine_core::h1::{HeaderTemplate, Reuse, write_request};
use write_rings::{DEFAULT_LINES, DEFAULTS, HOST, Request};

const RUNS: usize = 7;
const BLOCKS_PER_RUN: u32 = 4;
const ITERATIONS_PER_BLOCK: usize = 100_000;
const WARM_UP: Duration = Duration::from_millis(100);

/// A ring request with the framing lines the writer chose for it.
struct Framed {
    request: Request,
    content_length: bool,
    close: bool,
}

fn writer(out: &mut Vec<u8>, framed: &Framed, template: &HeaderTemplate) {
    let _ =
        write_request(out, &framed.request.parts(), template, HOST).expect("valid bench request");
}

/// The copy floor (note §8.1): the writer's bytes, appended in the same order, without any check.
fn floor(out: &mut Vec<u8>, framed: &Framed, template_bytes: &[u8]) {
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

/// Takes each request's framing from the writer's output and checks that the floor writes the same bytes; returns
/// the ring and its mean head length.
fn frame(
    ring: Vec<Request>,
    template: &HeaderTemplate,
    template_bytes: &[u8],
) -> (Vec<Framed>, usize) {
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
            floor(&mut copy, &framed, template_bytes);
            assert_eq!(copy, out, "the floor writes the writer's bytes");
            head += head_len;
            framed
        })
        .collect();
    let mean = head / framed.len();
    (framed, mean)
}

/// Nanoseconds per request over `ITERATIONS_PER_BLOCK` requests cycling through `ring`.
fn time_block(ring: &[Framed], out: &mut Vec<u8>, write: impl Fn(&mut Vec<u8>, &Framed)) -> f64 {
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

/// Interleaved runs of writer and floor; prints one JSON line per run and returns the medians and the ratio range.
fn measure(
    name: &str,
    ring: &[Framed],
    template: &HeaderTemplate,
    template_bytes: &[u8],
) -> [f64; 5] {
    let mut out = Vec::new();
    let ours = |out: &mut Vec<u8>, r: &Framed| writer(out, r, template);
    let copy = |out: &mut Vec<u8>, r: &Framed| floor(out, r, template_bytes);
    let start = Instant::now();
    while start.elapsed() < WARM_UP {
        time_block(ring, &mut out, ours);
        time_block(ring, &mut out, copy);
    }
    let (mut writer_ns, mut floor_ns, mut ratios) = (Vec::new(), Vec::new(), Vec::new());
    for run in 0..RUNS {
        let (mut a, mut b) = (0.0, 0.0);
        for _ in 0..BLOCKS_PER_RUN {
            a += time_block(ring, &mut out, ours);
            b += time_block(ring, &mut out, copy);
        }
        let (a, b) = (a / f64::from(BLOCKS_PER_RUN), b / f64::from(BLOCKS_PER_RUN));
        println!(
            "{{\"set\":\"{name}\",\"run\":{run},\"writer_ns\":{a:.2},\"floor_ns\":{b:.2},\"ratio\":{:.4}}}",
            a / b
        );
        writer_ns.push(a);
        floor_ns.push(b);
        ratios.push(a / b);
    }
    let min = ratios.iter().copied().fold(f64::INFINITY, f64::min);
    let max = ratios.iter().copied().fold(0.0, f64::max);
    [
        median(&mut writer_ns),
        median(&mut floor_ns),
        median(&mut ratios),
        min,
        max,
    ]
}

fn main() {
    let template = HeaderTemplate::new(HOST, DEFAULTS, DEFAULT_LINES, &[]).expect("valid template");
    let template_bytes = [&b"host: "[..], HOST, b"\r\n", DEFAULTS].concat();
    let sets = [
        ("get", write_rings::get()),
        ("post1k", write_rings::post1k()),
        ("varied", write_rings::varied()),
    ];
    let mut rows = Vec::new();
    for (name, ring) in sets {
        let (ring, head) = frame(ring, &template, &template_bytes);
        rows.push((name, head, measure(name, &ring, &template, &template_bytes)));
    }
    println!();
    println!(
        "write_request vs copy floor; {RUNS} runs x {BLOCKS_PER_RUN} blocks, median of per-run ratios"
    );
    println!(
        "| set | mean head B | writer ns (median) | floor ns (median) | ratio median | ratio min | ratio max |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (name, head, [w, f, ratio, min, max]) in rows {
        println!("| `{name}` | {head} | {w:.1} | {f:.1} | {ratio:.3} | {min:.3} | {max:.3} |");
    }
}
