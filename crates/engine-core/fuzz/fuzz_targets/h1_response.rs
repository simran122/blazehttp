//! `ResponseParser` fuzz target (`docs/notes/response-parser.md` §9.7). Inputs are structure-aware multi-message
//! exchanges (0–12 interim heads and a final head built from framing-relevant names and values, optional filler up to
//! the 64 KiB limits, optional byte mutations), limit-boundary exchanges, or raw bytes. Properties:
//! 1. no panic (debug assertions and overflow checks are on in this crate's release profile);
//! 2. the outcome is the same for the whole input, for arbitrary reads and for 1-byte reads;
//! 3. every head we accept, httparse (strict default config) accepts with the same status, version, fields and length;
//! 4. a reference model of RFC 9112 §6.3 plus design §5.2 over httparse's fields agrees with us: same framing and
//!    keep-alive when we accept; when we reject, the same error class (or `MalformedResponse` for a byte-level rule
//!    httparse does not check); `SmugglingRisk` from us only where the model says so;
//! 5. no over-strictness: an unmutated structured exchange whose bytes are all valid, which the model accepts and
//!    which fits the limits, is accepted.
//!
//! Run with the dictionary and value profile: `cargo +nightly-2026-11-06 fuzz run h1_response -- -dict=h1_response.dict
//! -use_value_profile=1` (and `-max_len=70000 -len_control=0` for the limit paths).

#![no_main]

use std::mem::MaybeUninit;

use arbitrary::Arbitrary;
use engine_core::generated::spec::{ErrorCode, H1_LIMITS, Method, ProtocolDetail, TooLargeDetail};
use engine_core::h1::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
use libfuzzer_sys::fuzz_target;

const MALFORMED: ErrorCode = ErrorCode::Protocol(ProtocolDetail::MalformedResponse);
const SMUGGLING: ErrorCode = ErrorCode::Protocol(ProtocolDetail::SmugglingRisk);
const TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);
/// Generated filler stops here: past the 64 KiB head limit, below libFuzzer's large `-max_len` runs.
const FILLER_CAP: usize = 70_000;

/// Optional whitespace: bits 0–1 = count (0–3), bit 2.. = which of them are HTAB.
#[derive(Arbitrary, Debug, Clone, Copy)]
struct Ows(u8);

impl Ows {
    fn write(self, out: &mut Vec<u8>) {
        for i in 0..(self.0 & 3) {
            out.push(if self.0 >> (2 + i) & 1 == 1 {
                b'\t'
            } else {
                b' '
            });
        }
    }
}

#[derive(Arbitrary, Debug)]
enum ConnToken {
    Close,
    KeepAlive,
    Upgrade,
    Closed,
    Empty,
}

/// Field values that reach the framing and `Connection` rules, plus raw bytes.
#[derive(Arbitrary, Debug)]
enum Value {
    /// `chunked` with a per-letter case mask and OWS around it.
    Chunked {
        case_mask: u8,
        lead: Ows,
        trail: Ows,
    },
    /// Comma-separated `Connection` options, each with OWS.
    Conn {
        tokens: Vec<(ConnToken, Ows)>,
    },
    /// A decimal with leading zeros.
    Digits {
        zeros: u8,
        n: u64,
    },
    /// `u64::MAX` + 1.
    Overflow,
    Raw(Vec<u8>),
}

impl Value {
    fn write(&self, out: &mut Vec<u8>) {
        match self {
            Self::Chunked {
                case_mask,
                lead,
                trail,
            } => {
                lead.write(out);
                for (i, b) in b"chunked".iter().enumerate() {
                    out.push(if case_mask >> i & 1 == 1 {
                        b.to_ascii_uppercase()
                    } else {
                        *b
                    });
                }
                trail.write(out);
            }
            Self::Conn { tokens } => {
                for (i, (token, ows)) in tokens.iter().take(6).enumerate() {
                    if i > 0 {
                        out.push(b',');
                    }
                    ows.write(out);
                    out.extend_from_slice(match token {
                        ConnToken::Close => b"close".as_slice(),
                        ConnToken::KeepAlive => b"keep-alive",
                        ConnToken::Upgrade => b"upgrade",
                        ConnToken::Closed => b"closed",
                        ConnToken::Empty => b"",
                    });
                }
            }
            Self::Digits { zeros, n } => {
                out.extend(std::iter::repeat_n(b'0', usize::from(zeros % 4)));
                out.extend_from_slice(n.to_string().as_bytes());
            }
            Self::Overflow => out.extend_from_slice(b"18446744073709551616"),
            Self::Raw(bytes) => out.extend_from_slice(bytes),
        }
    }

    fn is_valid_field_content(&self) -> bool {
        match self {
            Self::Raw(bytes) => bytes
                .iter()
                .all(|&b| b == b'\t' || (b' '..=b'~').contains(&b) || b >= 0x80),
            Self::Chunked { .. } | Self::Conn { .. } | Self::Digits { .. } | Self::Overflow => true,
        }
    }
}

const NAMES: &[&[u8]] = &[
    b"Content-Length",
    b"content-length",
    b"cOnTeNt-LeNgTh",
    b"Transfer-Encoding",
    b"transfer-encoding",
    b"TRANSFER-ENCODING",
    b"Connection",
    b"CONNECTION",
    b"Proxy-Connection",
    b"Keep-Alive",
    b"Date",
    b"x-a",
];
const STATUSES: &[u16] = &[
    100, 101, 103, 150, 199, 200, 204, 299, 304, 404, 599, 600, 99,
];

#[derive(Arbitrary, Debug)]
struct Field {
    name: u8,
    value: Value,
}

/// `count` lines `x-f: aaa…` of `len` value bytes each, capped at `FILLER_CAP` bytes in all.
#[derive(Arbitrary, Debug)]
struct Filler {
    count: u8,
    len: u16,
}

#[derive(Arbitrary, Debug)]
enum Reason {
    Phrase,
    /// CRLF straight after the code.
    Empty,
    /// No SP between the code and the reason (malformed).
    Glued,
}

#[derive(Arbitrary, Debug)]
struct Head {
    http10: bool,
    status: u8,
    reason: Reason,
    fields: Vec<Field>,
    filler: Option<Filler>,
}

impl Head {
    fn is_valid_bytes(&self) -> bool {
        !matches!(self.reason, Reason::Glued)
            && self.fields.iter().all(|f| f.value.is_valid_field_content())
    }
}

#[derive(Arbitrary, Debug)]
enum Mutation {
    Replace(u16, u8),
    Insert(u16, u8),
    Delete(u16),
}

/// Exchanges a few bytes either side of a limit (design §5.2), the structure-aware form of seeding the corpus with the
/// `head-65537`, `interim-plus-final-65537` and `status-line-8193` vectors.
#[derive(Arbitrary, Debug)]
enum Boundary {
    /// A final head of 65,536 + `delta` bytes.
    Head,
    /// A 32,768-byte `103` head and a final head of 32,768 + `delta` bytes.
    InterimPlusFinal,
    /// A status line of 8,192 + `delta` bytes, CRLF included.
    StatusLine,
    /// 100 + `delta` fields.
    Fields,
    /// 10 + `delta` interim heads.
    Interim,
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
    Boundary {
        kind: Boundary,
        delta: i8,
    },
}

#[derive(Arbitrary, Debug)]
struct Input {
    head_request: bool,
    shape: Shape,
    reads: Vec<u16>,
}

fn write_head(out: &mut Vec<u8>, head: &Head, interim: bool) {
    let mut status = STATUSES[usize::from(head.status) % STATUSES.len()];
    if interim && status >= 200 {
        status = 100 + status % 100;
    }
    out.extend_from_slice(if head.http10 {
        b"HTTP/1.0"
    } else {
        b"HTTP/1.1"
    });
    let line = match head.reason {
        Reason::Phrase => format!(" {status} R\r\n"),
        Reason::Empty => format!(" {status}\r\n"),
        Reason::Glued => format!(" {status}R\r\n"),
    };
    out.extend_from_slice(line.as_bytes());
    for field in head.fields.iter().take(H1_LIMITS.max_header_fields + 10) {
        out.extend_from_slice(NAMES[usize::from(field.name) % NAMES.len()]);
        out.extend_from_slice(b": ");
        field.value.write(out);
        out.extend_from_slice(b"\r\n");
    }
    if let Some(filler) = &head.filler {
        let len = usize::from(filler.len);
        for _ in 0..filler.count {
            if out.len() + len > FILLER_CAP {
                break;
            }
            out.extend_from_slice(b"x-f: ");
            out.extend(std::iter::repeat_n(b'a', len));
            out.extend_from_slice(b"\r\n");
        }
    }
    out.extend_from_slice(b"\r\n");
}

/// A head of exactly `total` bytes: `status_line`, `x-fill-NNNN: a…` lines of ≤ 1,000 bytes, the blank line.
fn sized_head(status_line: &str, total: usize) -> Vec<u8> {
    let mut out = format!("{status_line}\r\n").into_bytes();
    let mut i = 0;
    while out.len() + 2 < total {
        let remaining = total - out.len() - 2;
        let line = if remaining >= 1016 {
            1000
        } else {
            remaining.max(16)
        };
        let prefix = format!("x-fill-{i:04}: ");
        out.extend_from_slice(prefix.as_bytes());
        out.extend(std::iter::repeat_n(
            b'a',
            line.saturating_sub(prefix.len() + 2).max(1),
        ));
        out.extend_from_slice(b"\r\n");
        i += 1;
    }
    out.extend_from_slice(b"\r\n");
    out
}

fn boundary(kind: &Boundary, delta: i8) -> Vec<u8> {
    let at = |base: usize| base.saturating_add_signed(isize::from(delta % 4));
    let final_ok = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n".as_slice();
    match kind {
        Boundary::Head => sized_head("HTTP/1.1 200 OK", at(H1_LIMITS.max_head_bytes)),
        Boundary::InterimPlusFinal => {
            let half = H1_LIMITS.max_head_bytes / 2;
            [
                sized_head("HTTP/1.1 103 Early Hints", half),
                sized_head("HTTP/1.1 200 OK", at(half)),
            ]
            .concat()
        }
        Boundary::StatusLine => {
            let reason =
                at(H1_LIMITS.max_status_line_bytes).saturating_sub("HTTP/1.1 200 \r\n".len());
            let mut out = b"HTTP/1.1 200 ".to_vec();
            out.extend(std::iter::repeat_n(b'r', reason));
            out.extend_from_slice(b"\r\n\r\n");
            out
        }
        Boundary::Fields => {
            let mut out = b"HTTP/1.1 200 OK\r\n".to_vec();
            for i in 0..at(H1_LIMITS.max_header_fields) {
                out.extend_from_slice(format!("x-fill-{i:04}: a\r\n").as_bytes());
            }
            out.extend_from_slice(b"\r\n");
            out
        }
        Boundary::Interim => {
            let mut out =
                b"HTTP/1.1 100 Continue\r\n\r\n".repeat(at(H1_LIMITS.max_interim_responses));
            out.extend_from_slice(final_ok);
            out
        }
    }
}

/// The exchange's bytes, and whether they are a valid unmutated structured exchange (property 5 applies).
fn bytes_of(shape: &Shape) -> (Vec<u8>, bool) {
    match shape {
        Shape::Raw(bytes) => (bytes.clone(), false),
        Shape::Boundary { kind, delta } => (boundary(kind, *delta), false),
        Shape::Messages {
            interim,
            last,
            body,
            mutations,
        } => {
            let mut out = Vec::new();
            let interim = &interim[..interim.len().min(H1_LIMITS.max_interim_responses + 2)];
            for head in interim {
                write_head(&mut out, head, true);
            }
            write_head(&mut out, last, false);
            out.extend_from_slice(body);
            let mutations = &mutations[..mutations.len().min(4)];
            for mutation in mutations {
                match *mutation {
                    Mutation::Replace(i, b) if !out.is_empty() => {
                        let i = usize::from(i) % out.len();
                        out[i] = b;
                    }
                    Mutation::Insert(i, b) => out.insert(usize::from(i) % (out.len() + 1), b),
                    Mutation::Delete(i) if !out.is_empty() => {
                        out.remove(usize::from(i) % out.len());
                    }
                    Mutation::Replace(..) | Mutation::Delete(_) => {}
                }
            }
            let valid =
                mutations.is_empty() && interim.iter().chain([last]).all(Head::is_valid_bytes);
            (out, valid)
        }
    }
}

/// Each accepted head with the byte offset of its message.
#[derive(Debug, PartialEq, Eq)]
enum Accepted {
    Interim { start: usize, len: usize },
    Final { start: usize, head: ResponseHead },
}

/// What the parser made of an exchange.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    accepted: Vec<Accepted>,
    error: Option<ErrorCode>,
}

/// Feeds `input` in `reads` (increasing lengths), then EOF.
fn exchange(input: &[u8], method: Method, reads: impl IntoIterator<Item = usize>) -> Outcome {
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
                    return Outcome {
                        accepted,
                        error: None,
                    };
                }
                Err(e) => {
                    return Outcome {
                        accepted,
                        error: Some(e),
                    };
                }
            }
        }
    }
    Outcome {
        accepted,
        error: None,
    }
}

/// What httparse saw in one head.
struct Oracle {
    len: usize,
    status: u16,
    http10: bool,
    /// Length of the status line including CRLF.
    status_line_len: usize,
    fields: Vec<(String, Vec<u8>)>,
}

fn httparse_head(buf: &[u8]) -> Option<Oracle> {
    // One more slot than our limit, so the model (not httparse) decides a head with too many fields.
    let mut storage =
        [const { MaybeUninit::<httparse::Header>::uninit() }; H1_LIMITS.max_header_fields + 1];
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
        status_line_len: buf.iter().position(|&b| b == b'\n')? + 1,
        fields: response
            .headers
            .iter()
            .map(|h| (h.name.to_owned(), h.value.trim_ascii().to_vec()))
            .collect(),
    })
}

/// Framing facts the model collects from one head's fields.
#[derive(Default)]
struct FieldFacts {
    length: Option<u64>,
    chunked: bool,
    keep_alive_option: bool,
    /// A `Connection: close` option in this head.
    close: bool,
}

impl FieldFacts {
    /// Applies the count, `Content-Length`, `Transfer-Encoding` and `Connection` rules to the fields in order.
    fn scan(head: &Oracle, interim: bool) -> Result<Self, ErrorCode> {
        let mut facts = Self::default();
        for (index, (name, value)) in head.fields.iter().enumerate() {
            if index >= H1_LIMITS.max_header_fields {
                return Err(MALFORMED);
            }
            if name.eq_ignore_ascii_case("content-length") {
                facts.content_length(value, interim)?;
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                facts.transfer_encoding(value, head.http10, interim)?;
            } else if name.eq_ignore_ascii_case("connection") {
                facts.connection(value);
            }
        }
        Ok(facts)
    }

    fn content_length(&mut self, value: &[u8], interim: bool) -> Result<(), ErrorCode> {
        if self.length.is_some() || self.chunked {
            return Err(SMUGGLING);
        }
        let digits = !value.is_empty() && value.iter().all(u8::is_ascii_digit);
        let n: u64 = std::str::from_utf8(value)
            .ok()
            .filter(|_| digits)
            .and_then(|v| v.parse().ok())
            .ok_or(MALFORMED)?;
        if interim && n != 0 {
            return Err(SMUGGLING);
        }
        self.length = Some(n);
        Ok(())
    }

    fn transfer_encoding(
        &mut self,
        value: &[u8],
        http10: bool,
        interim: bool,
    ) -> Result<(), ErrorCode> {
        if self.length.is_some()
            || self.chunked
            || http10
            || interim
            || !value.eq_ignore_ascii_case(b"chunked")
        {
            return Err(SMUGGLING);
        }
        self.chunked = true;
        Ok(())
    }

    fn connection(&mut self, value: &[u8]) {
        for option in value.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
            self.close |= option.eq_ignore_ascii_case(b"close");
            self.keep_alive_option |= option.eq_ignore_ascii_case(b"keep-alive");
        }
    }
}

/// Reference model of the framing and count rules over one head's fields, in field order like the parser, returning
/// the error class. `close` is sticky across the exchange: it carries a `Connection: close` from earlier heads and
/// takes this head's (`keep-alive` counts for this head only).
fn model(
    head: &Oracle,
    interim_index: usize,
    head_request: bool,
    close: &mut bool,
) -> Result<(Framing, bool), ErrorCode> {
    let status = head.status;
    let interim = (100..200).contains(&status);
    if !(100..=599).contains(&status)
        || status == 101
        || (interim && interim_index >= H1_LIMITS.max_interim_responses)
    {
        return Err(MALFORMED);
    }
    let FieldFacts {
        length,
        chunked,
        keep_alive_option,
        close: close_option,
    } = FieldFacts::scan(head, interim)?;
    *close |= close_option;
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
    Ok((
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

/// A head httparse completed but we did not accept: we must have rejected it for a reason the model or the limits
/// give (we are stricter, never laxer).
fn check_rejection(
    verdict: Result<(Framing, bool), ErrorCode>,
    fits: bool,
    error: ErrorCode,
    valid: bool,
) {
    match verdict {
        _ if !fits => assert!(
            error == TOO_LARGE || error == MALFORMED || error == SMUGGLING,
            "{error:?}"
        ),
        Err(class) => assert!(
            error == class || error == MALFORMED,
            "error class: ours {error:?}, model {class:?}"
        ),
        Ok(_) => {
            assert_ne!(error, SMUGGLING, "smuggling-risk where the model accepts");
            assert!(
                !valid,
                "over-strict: rejected a valid exchange the model accepts ({error:?})"
            );
        }
    }
}

/// Properties 3–5 for the whole-input outcome.
fn check_against_oracle(input: &[u8], head_request: bool, valid: bool, outcome: &Outcome) {
    let Outcome { accepted, error } = outcome;
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
        let fits = start + oracle.len <= H1_LIMITS.max_head_bytes
            && oracle.status_line_len <= H1_LIMITS.max_status_line_bytes;
        let Some(next) = next else {
            let error = error.expect("a complete head was neither accepted nor rejected");
            check_rejection(verdict, fits, error, valid);
            return;
        };
        let verdict = verdict.expect("the model accepts every head we accept");
        assert!(fits, "accepted a head over the limits");
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
                return;
            }
        }
    }
}

fuzz_target!(|data: Input| {
    let (input, valid) = bytes_of(&data.shape);
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

    if let Some(e) = whole.error {
        assert!(ErrorCode::ALL_SAMPLES.contains(&e), "{e:?}");
    }
    check_against_oracle(&input, data.head_request, valid, &whole);
});
