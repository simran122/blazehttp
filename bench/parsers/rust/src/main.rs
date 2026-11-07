//! Rust harness of the cross-language response-head parser bench: blazehttp `ResponseParser` (alone and with the
//! per-field split) and httparse 1.10.1. Command line and output protocol: `bench/parsers/README.md`.

mod engines;

use std::alloc::{GlobalAlloc, Layout, System};
use std::fmt::Write as _;
use std::hint::black_box;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use engines::{Engine, Facts, Length};

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
fn split_heads(bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut heads = Vec::new();
    let mut rest = bytes;
    while let Some(end) = rest.windows(4).position(|w| w == b"\r\n\r\n") {
        let (head, tail) = rest.split_at(end + 4);
        heads.push(head);
        rest = tail;
    }
    if heads.is_empty() || !rest.is_empty() {
        return Err(
            "a workload is one or more heads, each ending at its first CRLF CRLF".to_owned(),
        );
    }
    Ok(heads)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            write!(s, "{b:02x}").expect("writing to a String cannot fail");
            s
        })
}

/// Folds the facts into one number so the timed work cannot be optimised away.
fn fold(f: Facts) -> u64 {
    let length = match f.length {
        Length::Bytes(n) => n,
        Length::Chunked => 1 << 40,
        Length::Unknown => 0,
    };
    u64::from(f.status) + f.head_len as u64 + length + u64::from(f.keep_alive)
}

fn check(engine: Engine, heads: &[&[u8]], mut parse: impl FnMut(&[u8]) -> Facts) {
    for head in heads {
        let f = parse(head);
        let length = match f.length {
            Length::Bytes(n) => n.to_string(),
            Length::Chunked => "chunked".to_owned(),
            Length::Unknown => "-".to_owned(),
        };
        let mut fields = Vec::new();
        engine.for_each_field(head, |name, value| {
            fields.push(format!("field {} {}", hex(name), hex(value)));
        });
        println!(
            "head {} {} {length} {} {}",
            f.status,
            f.head_len,
            u8::from(f.keep_alive),
            fields.len()
        );
        for line in fields {
            println!("{line}");
        }
    }
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    for i in 0..ALLOC_SAMPLE {
        black_box(parse(black_box(heads[i % heads.len()])));
    }
    let after = ALLOCATIONS.load(Ordering::Relaxed);
    #[expect(clippy::cast_precision_loss, reason = "counts far below 2^52")]
    let per_head = (after - before) as f64 / ALLOC_SAMPLE as f64;
    println!("allocs {per_head}");
}

/// Parses `BLOCK` heads cycling through `heads`, starting at `*next`; returns a value derived from every result.
fn block(heads: &[&[u8]], next: &mut usize, parse: &mut impl FnMut(&[u8]) -> Facts) -> u64 {
    let mut sink = 0_u64;
    for _ in 0..BLOCK {
        sink = sink.wrapping_add(fold(parse(black_box(heads[*next]))));
        *next = if *next + 1 == heads.len() {
            0
        } else {
            *next + 1
        };
    }
    sink
}

fn time(heads: &[&[u8]], run: Duration, mut parse: impl FnMut(&[u8]) -> Facts) {
    let mut next = 0;
    let mut sink = 0_u64;
    let warm = Instant::now();
    while warm.elapsed() < WARM_UP {
        sink = sink.wrapping_add(block(heads, &mut next, &mut parse));
    }
    let mut parsed = 0_u64;
    let start = Instant::now();
    while start.elapsed() < run {
        sink = sink.wrapping_add(block(heads, &mut next, &mut parse));
        parsed += BLOCK as u64;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations and counts far below 2^52"
    )]
    let ns = start.elapsed().as_nanos() as f64 / parsed as f64;
    println!("ns {ns} {sink}");
}

fn feed(engine: Engine, heads: &[&[u8]]) -> Result<(), String> {
    let [input] = heads else {
        return Err("a feed workload holds exactly one head".to_owned());
    };
    let start = Instant::now();
    let (mode, head_len) = engine.feed(input);
    let ns = start.elapsed().as_nanos();
    println!("feed {ns} {mode} {head_len}");
    Ok(())
}

/// One mode for one engine; `parse` is the engine's timed function, so every loop calls it directly.
fn run(
    engine: Engine,
    args: &[String],
    heads: &[&[u8]],
    parse: impl FnMut(&[u8]) -> Facts,
) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("check") => check(engine, heads, parse),
        Some("time") => {
            let ms = args
                .get(3)
                .and_then(|v| v.parse::<u64>().ok())
                .filter(|&ms| ms > 0)
                .ok_or("time needs a positive integer run time in ms")?;
            time(heads, Duration::from_millis(ms), parse);
        }
        Some("feed") => feed(engine, heads)?,
        _ => return Err("unknown mode".to_owned()),
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = (|| {
        let engine = args
            .first()
            .and_then(|n| Engine::from_name(n))
            .ok_or("usage: <blazehttp|blazehttp+fields|httparse> <check|time|feed> <workload file> [run ms]")?;
        let path = args.get(2).ok_or("missing workload file")?;
        let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
        let heads = split_heads(&bytes)?;
        match engine {
            Engine::Blazehttp => run(engine, &args, &heads, engines::parse_blazehttp),
            Engine::BlazehttpFields => {
                let mut spans = engines::NO_FIELDS;
                run(engine, &args, &heads, |head| {
                    engines::parse_blazehttp_fields(head, &mut spans)
                })
            }
            Engine::Httparse => run(engine, &args, &heads, engines::parse_httparse),
        }
    })();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
