//! `ResponseParser` fuzz target (`docs/notes/response-parser.md` §9.7). Properties, for multi-message inputs (0–11
//! interim heads and a final head, optionally mutated) and raw bytes:
//! 1. no panic (debug assertions and overflow checks are on in this crate's release profile);
//! 2. the outcome is the same for the whole input, for arbitrary reads and for 1-byte reads;
//! 3. every head we accept, httparse (strict default config) accepts with the same status, version, fields and length;
//! 4. a reference model of RFC 9112 §6.3 plus design §5.2 over httparse's fields agrees with our framing and
//!    keep-alive, and whenever the model rejects a head, so do we.

#![no_main]

use std::mem::MaybeUninit;

use arbitrary::Arbitrary;
use engine_core::generated::spec::{ErrorCode, H1_LIMITS, Method};
use engine_core::h1::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
use libfuzzer_sys::fuzz_target;

#[derive(Arbitrary, Debug)]
struct Field {
    name: u8,
    value: Vec<u8>,
}

#[derive(Arbitrary, Debug)]
struct Head {
    http10: bool,
    status: u16,
    fields: Vec<Field>,
}

#[derive(Arbitrary, Debug)]
enum Mutation {
    Replace(u16, u8),
    Insert(u16, u8),
    Delete(u16),
}

#[derive(Arbitrary, Debug)]
enum Shape {
    Raw(Vec<u8>),
    Messages {
        interim: Vec<Head>,
        last: Head,
        body: Vec<u8>,
        mutations: Vec<Mutation>,
    },
}

#[derive(Arbitrary, Debug)]
struct Input {
    head_request: bool,
    shape: Shape,
    reads: Vec<u16>,
}

const NAMES: &[&[u8]] = &[
    b"Content-Length",
    b"content-length",
    b"Transfer-Encoding",
    b"transfer-encoding",
    b"Connection",
    b"Keep-Alive",
    b"Date",
    b"x-a",
];
const STATUSES: &[u16] = &[
    100, 101, 103, 150, 199, 200, 204, 299, 304, 404, 599, 600, 99,
];

fn write_head(out: &mut Vec<u8>, head: &Head, interim: bool) {
    let pick = usize::from(head.status) % STATUSES.len();
    let mut status = STATUSES[pick];
    if interim && status >= 200 {
        status = 100 + status % 100;
    }
    let version: &[u8] = if head.http10 {
        b"HTTP/1.0"
    } else {
        b"HTTP/1.1"
    };
    out.extend_from_slice(version);
    out.extend_from_slice(format!(" {status} R\r\n").as_bytes());
    for field in head.fields.iter().take(110) {
        out.extend_from_slice(NAMES[usize::from(field.name) % NAMES.len()]);
        out.extend_from_slice(b": ");
        out.extend_from_slice(&field.value);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"\r\n");
}

fn bytes_of(shape: &Shape) -> Vec<u8> {
    match shape {
        Shape::Raw(bytes) => bytes.clone(),
        Shape::Messages {
            interim,
            last,
            body,
            mutations,
        } => {
            let mut out = Vec::new();
            for head in interim.iter().take(12) {
                write_head(&mut out, head, true);
            }
            write_head(&mut out, last, false);
            out.extend_from_slice(body);
            for mutation in mutations.iter().take(4) {
                let at = |i: u16| usize::from(i) % (out.len() + 1);
                match *mutation {
                    Mutation::Replace(i, b) if !out.is_empty() => {
                        let i = usize::from(i) % out.len();
                        out[i] = b;
                    }
                    Mutation::Insert(i, b) => {
                        let i = at(i);
                        out.insert(i, b);
                    }
                    Mutation::Delete(i) if !out.is_empty() => {
                        out.remove(usize::from(i) % out.len());
                    }
                    Mutation::Replace(..) | Mutation::Delete(_) => {}
                }
            }
            out
        }
    }
}

/// Each accepted head with the byte offset of its message.
#[derive(Debug, PartialEq, Eq)]
enum Accepted {
    Interim { start: usize, len: usize },
    Final { start: usize, head: ResponseHead },
}

/// Feeds `input` in `reads` (increasing lengths), then EOF; the outcome is the accepted heads and the error, if any.
fn exchange(
    input: &[u8],
    method: Method,
    reads: impl IntoIterator<Item = usize>,
) -> (Vec<Accepted>, Option<ErrorCode>) {
    let mut parser = ResponseParser::new(method);
    let mut accepted = Vec::new();
    let mut start = 0;
    for available in reads {
        loop {
            match parser.parse(&input[start..available]) {
                Ok(ParseStatus::Partial) => break,
                Ok(ParseStatus::Interim { consumed }) => {
                    accepted.push(Accepted::Interim {
                        start,
                        len: consumed,
                    });
                    start += consumed;
                }
                Ok(ParseStatus::Complete(head)) => {
                    accepted.push(Accepted::Final { start, head });
                    return (accepted, None);
                }
                Err(e) => return (accepted, Some(e)),
            }
        }
    }
    (accepted, None)
}

/// What httparse saw in one head.
struct Oracle {
    len: usize,
    status: u16,
    http10: bool,
    fields: Vec<(String, Vec<u8>)>,
}

fn httparse_head(buf: &[u8]) -> Option<Oracle> {
    let mut storage = [const { MaybeUninit::<httparse::Header>::uninit() }; 100];
    let mut response = httparse::Response::new(&mut []);
    let status = httparse::ParserConfig::default()
        .parse_response_with_uninit_headers(&mut response, buf, &mut storage)
        .ok()?;
    let httparse::Status::Complete(len) = status else {
        return None;
    };
    Some(Oracle {
        len,
        status: response.code?,
        http10: response.version? == 0,
        fields: response
            .headers
            .iter()
            .map(|h| (h.name.to_owned(), h.value.trim_ascii().to_vec()))
            .collect(),
    })
}

/// Reference model of the framing rules over one head's fields; `None` means the head must be rejected.
/// `close` carries a `Connection: close` from earlier heads of the exchange and is updated.
fn model(
    head: &Oracle,
    interim_index: usize,
    head_request: bool,
    close: &mut bool,
) -> Option<(Framing, bool)> {
    let status = head.status;
    let interim = (100..200).contains(&status);
    if !(100..=599).contains(&status)
        || status == 101
        || (interim && interim_index >= 10)
        || head.fields.len() > 100
    {
        return None;
    }
    let mut length: Option<u64> = None;
    let mut chunked = false;
    let mut keep_alive_option = false;
    for (name, value) in &head.fields {
        if name.eq_ignore_ascii_case("content-length") {
            let digits = !value.is_empty() && value.iter().all(u8::is_ascii_digit);
            if length.is_some() || chunked {
                return None;
            }
            let n: u64 = std::str::from_utf8(value)
                .ok()
                .filter(|_| digits)?
                .parse()
                .ok()?;
            if interim && n != 0 {
                return None;
            }
            length = Some(n);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            if length.is_some()
                || chunked
                || head.http10
                || interim
                || !value.eq_ignore_ascii_case(b"chunked")
            {
                return None;
            }
            chunked = true;
        } else if name.eq_ignore_ascii_case("connection") {
            for option in value.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
                *close |= option.eq_ignore_ascii_case(b"close");
                keep_alive_option |= option.eq_ignore_ascii_case(b"keep-alive");
            }
        }
    }
    let bodiless = head_request || status == 204 || status == 304;
    let framing = match (bodiless, chunked, length) {
        (true, ..) => Framing::Empty,
        (false, true, _) => Framing::Chunked,
        (false, false, Some(n)) => Framing::ContentLength(n),
        (false, false, None) => Framing::CloseDelimited,
    };
    let persistent = !*close && (!head.http10 || keep_alive_option);
    let unsent_body = !head_request
        && (status == 204 || status == 304)
        && (chunked || length.is_some_and(|n| n > 0));
    Some((
        framing,
        persistent && framing != Framing::CloseDelimited && !unsent_body,
    ))
}

fn fields_of(block: &[u8]) -> Vec<(String, Vec<u8>)> {
    block
        .split(|&b| b == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| {
            let line = line.strip_suffix(b"\r").expect("CRLF");
            let colon = line.iter().position(|&b| b == b':').expect("a colon");
            let name = String::from_utf8(line[..colon].to_vec()).expect("token names are ASCII");
            (name, line[colon + 1..].trim_ascii().to_vec())
        })
        .collect()
}

/// Properties 3 and 4 for the whole-input outcome.
fn check_against_oracle(
    input: &[u8],
    head_request: bool,
    accepted: &[Accepted],
    error: Option<ErrorCode>,
) {
    let mut close = false;
    let mut start = 0;
    let mut interim_index = 0;
    let mut ours = accepted.iter();
    loop {
        let next = ours.next();
        let Some(oracle) = httparse_head(&input[start..]) else {
            assert!(
                next.is_none(),
                "httparse rejects or wants more where we accepted: {next:?}"
            );
            return;
        };
        let verdict = model(&oracle, interim_index, head_request, &mut close);
        let Some(next) = next else {
            // httparse completed a head we did not accept: we must have rejected it (we are stricter, never laxer).
            assert!(
                error.is_some(),
                "a complete head was neither accepted nor rejected"
            );
            return;
        };
        let verdict = verdict.expect("the model accepts every head we accept");
        match next {
            Accepted::Interim { start: at, len } => {
                assert_eq!((*at, *len), (start, oracle.len));
                assert!((100..200).contains(&oracle.status));
                interim_index += 1;
                start += len;
            }
            Accepted::Final { start: at, head } => {
                assert_eq!(*at, start);
                assert_eq!(head.head_len, oracle.len);
                assert_eq!(head.status, oracle.status);
                assert_eq!(head.version == HttpVersion::Http10, oracle.http10);
                assert_eq!(
                    fields_of(&input[start..][head.headers.clone()]),
                    oracle.fields
                );
                assert_eq!((head.framing, head.keep_alive), verdict);
                assert!(start + head.head_len <= H1_LIMITS.max_head_bytes);
                return;
            }
        }
    }
}

fuzz_target!(|data: Input| {
    let input = bytes_of(&data.shape);
    let method = if data.head_request {
        Method::Head
    } else {
        Method::Get
    };
    let whole = exchange(&input, method, [input.len()]);

    let mut reads: Vec<usize> = data
        .reads
        .iter()
        .map(|&r| usize::from(r) % (input.len() + 1))
        .collect();
    reads.push(input.len());
    reads.sort_unstable();
    assert_eq!(exchange(&input, method, reads), whole, "arbitrary reads");
    assert_eq!(
        exchange(&input, method, 1..=input.len()),
        whole,
        "1-byte reads"
    );

    if let Some(e) = whole.1 {
        assert!(ErrorCode::ALL_SAMPLES.contains(&e), "{e:?}");
    }
    check_against_oracle(&input, data.head_request, &whole.0, whole.1);
});
