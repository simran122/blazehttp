//! Development benchmark: `ResponseParser` vs httparse 1.10.1 plus the same framing-fact extraction
//! (`docs/notes/response-parser.md` §8.1). Run with `cargo run --release -p engine-core --example h1_parse_bench`.
//! Results are dev measurements (`docs/perf/journal.md`); P1a replaces this with the criterion bench `benches/h1_parse.rs`.
// removal: P1a (#23)

#[path = "../tests/support/fixtures.rs"]
mod fixtures;

use std::hint::black_box;
use std::mem::MaybeUninit;
use std::time::{Duration, Instant};

use engine_core::generated::spec::Method;
use engine_core::h1::{Framing, ParseStatus, ResponseParser};

const FIXTURES: [&str; 3] = ["1k.http", "1k-h20.http", "synthetic-h20.http"];
/// The varied ring (docs/notes/simd-scanning-research.md §7 item 1): `1k-h20.http` with shuffled field order and
/// value lengths scaled 0.5–1.5×, so the branch predictor cannot learn one head.
const VARIED_HEADS: usize = 256;
const VARIED: &str = "varied 1k-h20 ring";
const VARIANTS: usize = 64;
const RUNS: usize = 7;
const BLOCKS_PER_RUN: u32 = 4;
const ITERATIONS_PER_BLOCK: usize = 50_000;
const WARM_UP: Duration = Duration::from_millis(100);
const HTTPARSE_HEADERS: usize = 100;

/// `(needle, how many bytes after its start vary)`: date seconds, content length, ids and etag hex digits.
const VARYING: [(&[u8], usize); 5] = [
    (b"12:00:", 8),
    (b"content-length: ", 19),
    (b"x-request-id: ", 16),
    (b"traceparent: 00-", 18),
    (b"etag: W/\"34f-", 15),
];

/// 64 heads of the same length as `base`, differing in the digits named by `VARYING`, so no single head trains
/// the branch predictor.
fn variants(base: &[u8]) -> Vec<Vec<u8>> {
    (0..VARIANTS)
        .map(|v| {
            let mut head = base.to_vec();
            for (needle, end) in VARYING {
                let Some(at) = head.windows(needle.len()).position(|w| w == needle) else {
                    continue;
                };
                let digits = &mut head[at + needle.len()..at + end];
                for (i, digit) in digits.iter_mut().enumerate() {
                    if digit.is_ascii_hexdigit() {
                        let n = u8::try_from((v * 7 + i * 3) % 10).expect("< 10");
                        // Keep the first content-length digit non-zero (three-digit values).
                        *digit = if i == 0 && needle == b"content-length: " {
                            b'1' + n % 9
                        } else {
                            b'0' + n
                        };
                    }
                }
            }
            assert_eq!(head.len(), base.len());
            head
        })
        .collect()
}

/// xorshift64: a fixed, seedable sequence (no dependency) for the varied ring.
fn next(state: &mut u64) -> u64 {
    *state ^= *state << 13;
    *state ^= *state >> 7;
    *state ^= *state << 17;
    *state
}

fn varied(base: &[u8]) -> Vec<Vec<u8>> {
    let text = std::str::from_utf8(base).expect("ASCII fixture");
    let mut lines: Vec<&str> = text
        .trim_end_matches("\r\n")
        .split("\r\n")
        .skip(1)
        .collect();
    let mut state = 0x9E37_79B9_7F4A_7C15_u64;
    (0..VARIED_HEADS)
        .map(|_| {
            for i in (1..lines.len()).rev() {
                let j = usize::try_from(next(&mut state) % (i as u64 + 1)).expect("small");
                lines.swap(i, j);
            }
            let mut head = String::from("HTTP/1.1 200 OK\r\n");
            for line in &lines {
                let (name, value) = line.split_once(": ").expect("fixture field");
                let scale = 50 + next(&mut state) % 101; // percent, 50..=150
                let len = (value.len() * usize::try_from(scale).expect("small") / 100).max(1);
                let value: String = if name == "content-length" {
                    value.to_owned()
                } else {
                    value.chars().cycle().take(len).collect()
                };
                for part in [name, ": ", &value, "\r\n"] {
                    head.push_str(part);
                }
            }
            head.push_str("\r\n");
            head.into_bytes()
        })
        .collect()
}

fn ours(head: &[u8]) -> (Framing, bool) {
    match ResponseParser::new(Method::Get).parse(head) {
        Ok(ParseStatus::Complete(head)) => (head.framing, head.keep_alive),
        other => panic!("fixture did not parse: {other:?}"),
    }
}

/// httparse plus the same facts our parser folds: `Content-Length`, `Transfer-Encoding`, `Connection`.
fn httparse_with_facts(head: &[u8]) -> (Framing, bool) {
    let mut storage = [const { MaybeUninit::<httparse::Header>::uninit() }; HTTPARSE_HEADERS];
    let mut response = httparse::Response::new(&mut []);
    let status = httparse::ParserConfig::default()
        .parse_response_with_uninit_headers(&mut response, head, &mut storage)
        .expect("fixture parses");
    assert!(status.is_complete());
    let mut length = None;
    let mut chunked = false;
    let mut close = false;
    for header in response.headers.iter() {
        let value = header.value.trim_ascii();
        if header.name.eq_ignore_ascii_case("content-length") {
            assert!(length.is_none() && !chunked);
            length = Some(value.iter().try_fold(0_u64, |n, &b| {
                b.is_ascii_digit()
                    .then(|| n.checked_mul(10)?.checked_add(u64::from(b - b'0')))
                    .flatten()
            }));
        } else if header.name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case(b"chunked");
        } else if header.name.eq_ignore_ascii_case("connection") {
            close |= value
                .split(|&b| b == b',')
                .any(|o| o.trim_ascii().eq_ignore_ascii_case(b"close"));
        }
    }
    let framing = match (chunked, length.flatten()) {
        (true, _) => Framing::Chunked,
        (false, Some(n)) => Framing::ContentLength(n),
        (false, None) => Framing::CloseDelimited,
    };
    (framing, !close && framing != Framing::CloseDelimited)
}

/// Nanoseconds per parse over `ITERATIONS_PER_BLOCK` parses cycling through `heads`.
fn time_block(heads: &[Vec<u8>], parse: fn(&[u8]) -> (Framing, bool)) -> f64 {
    let start = Instant::now();
    for i in 0..ITERATIONS_PER_BLOCK {
        black_box(parse(black_box(&heads[i % heads.len()])));
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "durations and counts far below 2^52"
    )]
    let ns = start.elapsed().as_nanos() as f64 / ITERATIONS_PER_BLOCK as f64;
    ns
}

fn warm_up(heads: &[Vec<u8>], parse: fn(&[u8]) -> (Framing, bool)) {
    let start = Instant::now();
    while start.elapsed() < WARM_UP {
        time_block(heads, parse);
    }
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

/// Feeds `input` one byte at a time, as a connection would on 1-byte reads; returns the elapsed time.
fn feed_ours(input: &[u8]) -> Duration {
    let start = Instant::now();
    let mut parser = ResponseParser::new(Method::Get);
    for end in 1..=input.len() {
        match parser.parse(black_box(&input[..end])) {
            Ok(ParseStatus::Partial) => {}
            Ok(ParseStatus::Complete(_)) if end == input.len() => {}
            other => panic!("feed failed at {end}: {other:?}"),
        }
    }
    start.elapsed()
}

/// httparse re-parses from the start on every read (it is not resumable).
fn feed_httparse(input: &[u8]) -> Duration {
    let start = Instant::now();
    for end in 1..=input.len() {
        let mut storage = [const { MaybeUninit::<httparse::Header>::uninit() }; HTTPARSE_HEADERS];
        let mut response = httparse::Response::new(&mut []);
        let status = httparse::ParserConfig::default().parse_response_with_uninit_headers(
            &mut response,
            black_box(&input[..end]),
            &mut storage,
        );
        assert_eq!(
            status.expect("valid prefix").is_complete(),
            end == input.len()
        );
    }
    start.elapsed()
}

fn feeds() -> [(&'static str, Vec<u8>); 4] {
    let status = b"HTTP/1.1 200 OK\r\n";
    let value = [
        &status[..],
        b"x-v: ",
        &vec![b'v'; 60_000],
        b"\r\ncontent-length: 0\r\n\r\n",
    ]
    .concat();
    let mut fields = status.to_vec();
    for i in 0..99 {
        let line = format!("x-fill-{i:04}: {}\r\n", "a".repeat(600 - 15));
        fields.extend_from_slice(line.as_bytes());
    }
    fields.extend_from_slice(b"\r\n");
    let name = [&status[..], &vec![b'x'; 60_000], b": 1\r\n\r\n"].concat();
    let reason_len = 8192 - "HTTP/1.1 200 ".len() - 2;
    let status_line = [&b"HTTP/1.1 200 "[..], &vec![b'r'; reason_len], b"\r\n\r\n"].concat();
    [
        ("(a) one 60,000-byte value", value),
        ("(b) 99 fields x 600 B", fields),
        ("(c) one 60,000-byte name", name),
        ("(d) 8,192-byte status line", status_line),
    ]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn main() {
    println!("ResponseParser vs httparse 1.10.1 + facts; {RUNS} runs, median of per-run ratios");
    println!(
        "| head | bytes | ours ns (median) | httparse ns (median) | ratio median | ratio min | ratio max |"
    );
    println!("|---|---|---|---|---|---|---|");
    let rings = FIXTURES
        .iter()
        .map(|&name| {
            let base = fixtures::h1_head(name);
            (name, base.len(), variants(&base))
        })
        .chain(std::iter::once({
            let heads = varied(&fixtures::h1_head("1k-h20.http"));
            let mean = heads.iter().map(Vec::len).sum::<usize>() / heads.len();
            (VARIED, mean, heads)
        }));
    for (name, bytes, heads) in rings {
        for head in &heads {
            assert_eq!(ours(head), httparse_with_facts(head), "{name}");
        }
        warm_up(&heads, ours);
        warm_up(&heads, httparse_with_facts);
        let (mut ours_ns, mut theirs_ns, mut ratios) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..RUNS {
            let (mut a, mut b) = (0.0, 0.0);
            for _ in 0..BLOCKS_PER_RUN {
                a += time_block(&heads, ours);
                b += time_block(&heads, httparse_with_facts);
            }
            ours_ns.push(a / f64::from(BLOCKS_PER_RUN));
            theirs_ns.push(b / f64::from(BLOCKS_PER_RUN));
            ratios.push(a / b);
        }
        let min = ratios.iter().copied().fold(f64::INFINITY, f64::min);
        let max = ratios.iter().copied().fold(0.0, f64::max);
        println!(
            "| `{name}` | {} | {:.1} | {:.1} | {:.3} | {min:.3} | {max:.3} |",
            bytes,
            median(&mut ours_ns),
            median(&mut theirs_ns),
            median(&mut ratios),
        );
    }
    println!();
    println!("| 1-byte feed | bytes | ours ms | httparse ms |");
    println!("|---|---|---|---|");
    for (i, (label, input)) in feeds().into_iter().enumerate() {
        let theirs = if i == 0 {
            format!("{:.2}", ms(feed_httparse(&input)))
        } else {
            "-".to_owned()
        };
        println!(
            "| {label} | {} | {:.3} | {theirs} |",
            input.len(),
            ms(feed_ours(&input))
        );
    }
}
