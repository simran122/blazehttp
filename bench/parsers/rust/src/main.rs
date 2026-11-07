//! Rust harness of the cross-language response-head parser bench: blazehttp `ResponseParser` and httparse 1.10.1.
//! Command line and output protocol: `bench/parsers/README.md` ("Harness protocol").

mod engines;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use engines::Engine;

const WARM_UP: Duration = Duration::from_millis(100);
const BLOCK: usize = 1_000;
const ALLOC_SAMPLE: usize = 10_000;

/// Counts allocations so `check` can report allocations per head (design §16.2 "allocations per request").
struct CountingAlloc;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

// SAFETY: forwards every call unchanged to the system allocator; the counter has no effect on memory.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract, which `System` shares.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: `ptr` was returned by `System.alloc` with this `layout`.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Splits a workload file into heads: each head ends at its first `\r\n\r\n`.
fn split_heads(bytes: &[u8]) -> Vec<&[u8]> {
    let mut heads = Vec::new();
    let mut rest = bytes;
    while let Some(end) = rest.windows(4).position(|w| w == b"\r\n\r\n") {
        let (head, tail) = rest.split_at(end + 4);
        heads.push(head);
        rest = tail;
    }
    assert!(rest.is_empty(), "trailing bytes after the last head");
    heads
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("writing to a String cannot fail");
            s
        })
}

fn check(engine: Engine, heads: &[&[u8]]) {
    for head in heads {
        let d = engine.describe(head);
        let length = d
            .content_length
            .map_or_else(|| "-".to_owned(), |n| n.to_string());
        println!(
            "head {} {} {length} {}",
            d.status,
            d.head_len,
            d.fields.len()
        );
        for (name, value) in &d.fields {
            println!("field {} {}", hex(name), hex(value));
        }
    }
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for i in 0..ALLOC_SAMPLE {
        black_box(engine.parse(black_box(heads[i % heads.len()])));
    }
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    #[expect(clippy::cast_precision_loss, reason = "counts far below 2^52")]
    let per_head = (after - before) as f64 / ALLOC_SAMPLE as f64;
    println!("allocs {per_head}");
}

/// Parses `BLOCK` heads cycling through `heads`, starting at `*next`; returns a value derived from every result.
fn block(engine: Engine, heads: &[&[u8]], next: &mut usize) -> u64 {
    let mut sink = 0_u64;
    for _ in 0..BLOCK {
        sink = sink.wrapping_add(engine.parse(black_box(heads[*next])));
        *next = (*next + 1) % heads.len();
    }
    sink
}

fn time(engine: Engine, heads: &[&[u8]], run: Duration) {
    let mut next = 0;
    let mut sink = 0_u64;
    let warm = Instant::now();
    while warm.elapsed() < WARM_UP {
        sink = sink.wrapping_add(block(engine, heads, &mut next));
    }
    let mut parsed = 0_u64;
    let start = Instant::now();
    while start.elapsed() < run {
        sink = sink.wrapping_add(block(engine, heads, &mut next));
        parsed += BLOCK as u64;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations and counts far below 2^52"
    )]
    let ns = start.elapsed().as_nanos() as f64 / parsed as f64;
    println!("ns {ns} {sink}");
}

fn feed(engine: Engine, heads: &[&[u8]]) {
    let [input] = heads else {
        panic!("a feed workload holds exactly one head");
    };
    let start = Instant::now();
    let (mode, head_len) = engine.feed(input);
    let ns = start.elapsed().as_nanos();
    println!("feed {ns} {mode} {head_len}");
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(engine), Some(mode), Some(path)) = (
        args.first().and_then(|n| Engine::from_name(n)),
        args.get(1),
        args.get(2),
    ) else {
        eprintln!("usage: <blazehttp|httparse> <check|time|feed> <workload file> [run ms]");
        return ExitCode::FAILURE;
    };
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let heads = split_heads(&bytes);
    match mode.as_str() {
        "check" => check(engine, &heads),
        "time" => {
            let ms = args.get(3).and_then(|v| v.parse().ok()).unwrap_or(300);
            time(engine, &heads, Duration::from_millis(ms));
        }
        "feed" => feed(engine, &heads),
        _ => {
            eprintln!("unknown mode {mode}");
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
