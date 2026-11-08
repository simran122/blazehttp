//! Development benchmark: `write_request` vs the copy floor (`docs/notes/request-writer.md` §8.1, §8.4). Run with
//! `cargo run --release -p engine-core --example h1_write_bench` (`-- --verify` runs the byte-equality checks only,
//! as CI does). The copy floor appends the same bytes with the same `extend_from_slice` sequence and the same framing
//! result (taken from the writer once, untimed) but no validation, no limit, authority, singleton or line-count
//! check, no checked length sum and no reservation, so writer − floor = validation + bookkeeping. Prints one JSON line
//! per run and a Markdown table. Results are dev measurements (`docs/perf/journal.md`).
//! P1a replaces it with the criterion bench `benches/h1_write.rs`.
// removal: P1a (#23)

#[path = "../tests/support/write_bench.rs"]
mod write_bench;
#[path = "../tests/support/write_rings.rs"]
mod write_rings;

use std::time::Instant;

use engine_core::h1::{HeaderTemplate, write_request};
use write_bench::{
    BLOCKS_PER_RUN, Framed, RUNS, WARM_UP, floor, frame, median, template_bytes, time_block,
};
use write_rings::{DEFAULT_LINES, DEFAULTS, HOST};

/// Interleaved runs of writer and floor; prints one JSON line per run and returns the medians and the ratio range.
fn measure(name: &str, ring: &[Framed], template: &HeaderTemplate) -> [f64; 5] {
    let template_bytes = template_bytes();
    let mut out = Vec::new();
    let ours = |out: &mut Vec<u8>, r: &Framed| {
        let _ =
            write_request(out, &r.request.parts(), template, HOST).expect("valid bench request");
    };
    let copy = |out: &mut Vec<u8>, r: &Framed| floor(out, r, &template_bytes);
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
    let sets = [
        ("get", write_rings::get()),
        ("post1k", write_rings::post1k()),
        ("varied", write_rings::varied()),
    ];
    let rings: Vec<_> = sets
        .into_iter()
        .map(|(name, ring)| (name, frame(ring, &template)))
        .collect();
    if write_bench::verify_only() {
        println!(
            "verified: the copy floor writes the writer's bytes for every request of every set"
        );
        return;
    }
    let rows: Vec<_> = rings
        .iter()
        .map(|(name, (ring, head))| (name, head, measure(name, ring, &template)))
        .collect();
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
