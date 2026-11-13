//! Development benchmark: `write_request` against its baselines (`docs/notes/request-writer.md` §8.1, §8.4;
//! `tests/support/write_bench.rs`): `memcpy` of the finished request (the lower bound), the unchecked appends (the
//! former "copy floor") and the writer's own append shape without checks. All four variants run in one binary, in
//! rotated order. `writer − shape` is the cost of the checks, `shape − appends` that of the append shape. Run with
//! `cargo run --release -p engine-core --example h1_write_bench` (`-- --verify` runs the byte-equality checks only,
//! as CI does); `bench/writer/run.sh` runs it in the pinned container. Prints one JSON line per run and a Markdown
//! table of medians. Results are dev measurements (`docs/perf/journal.md`).
//! P1a replaces it with the criterion bench `benches/h1_write.rs`.
// removal: P1a (#23)

#[path = "../tests/support/write_bench.rs"]
mod write_bench;
#[path = "../tests/support/write_rings.rs"]
mod write_rings;

use engine_core::h1::{HeaderTemplate, write_request};
use write_bench::{
    BLOCKS_PER_RUN, Framed, RUNS, frame, measure_variants, median, memcpy, template_bytes,
    time_block, unchecked_appends, writer_shape,
};
use write_rings::{DEFAULT_LINES, DEFAULTS, HOST};

const VARIANTS: [&str; 4] = ["writer", "shape", "appends", "memcpy"];

/// Medians over the runs: the four variants' ns, then `writer − shape`, `writer − appends` and `writer / appends`
/// (each computed per run), then the min and max of `writer / appends`.
fn measure(name: &str, ring: &[Framed], template: &HeaderTemplate) -> [f64; 9] {
    let template_bytes = template_bytes();
    let mut out = Vec::new();
    let [mut writer, mut shape, mut appends, mut copy] =
        measure_variants(name, VARIANTS, |variant| match variant {
            0 => time_block(ring, &mut out, |out, r: &Framed| {
                let _ = write_request(out, &r.request.parts(), template, HOST)
                    .expect("valid bench request");
            }),
            1 => time_block(ring, &mut out, |out, r| {
                writer_shape(out, r, &template_bytes);
            }),
            2 => time_block(ring, &mut out, |out, r| {
                unchecked_appends(out, r, &template_bytes);
            }),
            _ => time_block(ring, &mut out, memcpy),
        });
    let per_run = |f: fn(f64, f64) -> f64, a: &[f64], b: &[f64]| -> Vec<f64> {
        a.iter().zip(b).map(|(&x, &y)| f(x, y)).collect()
    };
    let mut checks = per_run(|w, s| w - s, &writer, &shape);
    let mut over_appends = per_run(|w, a| w - a, &writer, &appends);
    let mut ratios = per_run(|w, a| w / a, &writer, &appends);
    let min = ratios.iter().copied().fold(f64::INFINITY, f64::min);
    let max = ratios.iter().copied().fold(0.0, f64::max);
    [
        median(&mut writer),
        median(&mut shape),
        median(&mut appends),
        median(&mut copy),
        median(&mut checks),
        median(&mut over_appends),
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
            "verified: every baseline writes the writer's bytes for every request of every set"
        );
        return;
    }
    let rows: Vec<_> = rings
        .iter()
        .map(|(name, (ring, head))| (name, head, measure(name, ring, &template)))
        .collect();
    println!();
    println!(
        "write_request vs baselines; {RUNS} runs x {BLOCKS_PER_RUN} rotated blocks, medians (ns per request)"
    );
    println!(
        "| set | mean head B | writer | shape | appends | memcpy | writer − shape | writer − appends | \
         writer / appends (min–max) |"
    );
    println!("|---|---|---|---|---|---|---|---|---|");
    for (name, head, [w, s, a, m, checks, over, ratio, min, max]) in rows {
        println!(
            "| `{name}` | {head} | {w:.1} | {s:.1} | {a:.1} | {m:.1} | {checks:.1} | {over:.1} | \
             {ratio:.2} ({min:.2}–{max:.2}) |"
        );
    }
}
