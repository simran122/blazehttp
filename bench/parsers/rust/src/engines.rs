//! The Rust parsers, each through its fastest idiomatic API, doing the same work per head: validate the head, find
//! its end, and derive the framing facts (`Content-Length`, chunked, `Connection: close`).

use std::hint::black_box;
use std::mem::MaybeUninit;
use std::ops::Range;

use engine_core::generated::spec::{H1_LIMITS, Method};
use engine_core::h1::{Framing, ParseStatus, ResponseHead, ResponseParser};

/// Field slots for the array-filling parsers: our field limit, which also covers the 99-field feed input (b).
const FIELD_SLOTS: usize = H1_LIMITS.max_header_fields;

#[derive(Clone, Copy)]
pub enum Engine {
    /// `ResponseParser` alone: validated head, framing facts and the field-line block as one range.
    Blazehttp,
    /// `ResponseParser` plus [`split_fields`]: the per-field name/value slices httparse returns.
    BlazehttpFields,
    Httparse,
}

/// How the body is delimited, as every harness prints it.
#[derive(Clone, Copy)]
pub enum Length {
    Bytes(u64),
    Chunked,
    /// Neither `Content-Length` nor chunked (read to EOF).
    Unknown,
}

/// What the timed path produces for one head (printed by `check` as the `facts` line).
#[derive(Clone, Copy)]
pub struct Facts {
    pub status: u16,
    pub head_len: usize,
    pub length: Length,
    pub keep_alive: bool,
}

/// One field line as offsets into its head: the name and the value (OWS trimmed), the pair httparse's `Header` gives.
/// Offsets rather than slices, so a connection can keep one span array and reuse it for every response.
#[derive(Clone, Copy)]
pub struct FieldSpan {
    name: (usize, usize),
    value: (usize, usize),
}

impl FieldSpan {
    pub fn name<'b>(&self, head: &'b [u8]) -> &'b [u8] {
        &head[self.name.0..self.name.1]
    }

    pub fn value<'b>(&self, head: &'b [u8]) -> &'b [u8] {
        &head[self.value.0..self.value.1]
    }
}

/// Spans for the 100 fields a head may have.
pub type FieldSpans = [FieldSpan; FIELD_SLOTS];

pub const NO_FIELDS: FieldSpans = [FieldSpan {
    name: (0, 0),
    value: (0, 0),
}; FIELD_SLOTS];

const fn is_ows(b: u8) -> bool {
    b == b' ' || b == b'\t'
}

/// Splits the validated field lines `head[block]` (`ResponseHead::headers`) into `out` without allocating; returns
/// the number of fields. Each line is `name ":" OWS value OWS CRLF` (the parser guarantees it), so a line ends at its
/// first CR (found with `memchr`, vectorised) and its name at the first `:` (names are short tokens).
///
/// # Panics
///
/// When the block holds more fields than `out` has slots (the parser's field limit sizes `out`).
pub fn split_fields(head: &[u8], block: Range<usize>, out: &mut [FieldSpan]) -> usize {
    let mut pos = block.start;
    let mut count = 0;
    while let Some(cr) = memchr::memchr(b'\r', &head[pos..block.end]) {
        let end = pos + cr;
        let colon = pos
            + head[pos..end]
                .iter()
                .position(|&b| b == b':')
                .expect("validated field line");
        let mut value = (colon + 1, end);
        while value.0 < value.1 && is_ows(head[value.0]) {
            value.0 += 1;
        }
        while value.1 > value.0 && is_ows(head[value.1 - 1]) {
            value.1 -= 1;
        }
        out[count] = FieldSpan {
            name: (pos, colon),
            value,
        };
        count += 1;
        pos = end + 2;
    }
    count
}

fn length_of(framing: Framing) -> Length {
    match framing {
        Framing::ContentLength(n) => Length::Bytes(n),
        Framing::Chunked => Length::Chunked,
        Framing::CloseDelimited | Framing::Empty => Length::Unknown,
    }
}

fn ours(head: &[u8]) -> ResponseHead {
    match ResponseParser::new(Method::Get).parse(head) {
        Ok(ParseStatus::Complete(parsed)) => parsed,
        other => panic!("blazehttp did not parse the head: {other:?}"),
    }
}

fn ours_facts(parsed: &ResponseHead) -> Facts {
    Facts {
        status: parsed.status,
        head_len: parsed.head_len,
        length: length_of(parsed.framing),
        keep_alive: parsed.keep_alive,
    }
}

/// Timed path of [`Engine::Blazehttp`].
pub fn parse_blazehttp(head: &[u8]) -> Facts {
    ours_facts(&ours(head))
}

/// Timed path of [`Engine::BlazehttpFields`]: `spans` is the connection's reused span array.
pub fn parse_blazehttp_fields(head: &[u8], spans: &mut FieldSpans) -> Facts {
    let parsed = ours(head);
    let count = split_fields(head, parsed.headers.clone(), spans);
    black_box(&spans[..count]);
    ours_facts(&parsed)
}

fn httparse_parse<'h, 'b>(
    head: &'b [u8],
    storage: &'h mut [MaybeUninit<httparse::Header<'b>>; FIELD_SLOTS],
) -> Result<(httparse::Response<'h, 'b>, usize), httparse::Error> {
    let mut response = httparse::Response::new(&mut []);
    let status = httparse::ParserConfig::default().parse_response_with_uninit_headers(
        &mut response,
        head,
        storage,
    )?;
    let head_len = match status {
        httparse::Status::Complete(n) => n,
        httparse::Status::Partial => 0,
    };
    Ok((response, head_len))
}

/// The framing facts our parser folds, derived from httparse's slices (same rules as `examples/h1_parse_bench.rs`).
fn httparse_facts(response: &httparse::Response<'_, '_>, head_len: usize) -> Facts {
    let mut length = None;
    let mut chunked = false;
    let mut close = false;
    for header in response.headers.iter() {
        let value = header.value.trim_ascii();
        if header.name.eq_ignore_ascii_case("content-length") {
            length = value.iter().try_fold(0_u64, |n, &b| {
                b.is_ascii_digit()
                    .then(|| n.checked_mul(10)?.checked_add(u64::from(b - b'0')))
                    .flatten()
            });
        } else if header.name.eq_ignore_ascii_case("transfer-encoding") {
            chunked = value.eq_ignore_ascii_case(b"chunked");
        } else if header.name.eq_ignore_ascii_case("connection") {
            close |= value
                .split(|&b| b == b',')
                .any(|o| o.trim_ascii().eq_ignore_ascii_case(b"close"));
        }
    }
    let length = match (chunked, length) {
        (true, _) => Length::Chunked,
        (false, Some(n)) => Length::Bytes(n),
        (false, None) => Length::Unknown,
    };
    Facts {
        status: response.code.unwrap_or(0),
        head_len,
        length,
        keep_alive: !close && !matches!(length, Length::Unknown),
    }
}

/// Timed path of [`Engine::Httparse`].
pub fn parse_httparse(head: &[u8]) -> Facts {
    let mut storage = [const { MaybeUninit::uninit() }; FIELD_SLOTS];
    let (response, head_len) =
        httparse_parse(head, &mut storage).expect("httparse parses the head");
    httparse_facts(&response, head_len)
}

impl Engine {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "blazehttp" => Some(Self::Blazehttp),
            "blazehttp+fields" => Some(Self::BlazehttpFields),
            "httparse" => Some(Self::Httparse),
            _ => None,
        }
    }

    /// Calls `visit` with each field of the head, in order (the gate's view, outside the timed path).
    pub fn for_each_field(self, head: &[u8], mut visit: impl FnMut(&[u8], &[u8])) {
        match self {
            Self::Blazehttp | Self::BlazehttpFields => {
                let parsed = ours(head);
                let mut spans = NO_FIELDS;
                let count = split_fields(head, parsed.headers.clone(), &mut spans);
                for span in &spans[..count] {
                    visit(span.name(head), span.value(head));
                }
            }
            Self::Httparse => {
                let mut storage = [const { MaybeUninit::uninit() }; FIELD_SLOTS];
                let (response, _) =
                    httparse_parse(head, &mut storage).expect("httparse parses the head");
                for header in response.headers.iter() {
                    visit(header.name.as_bytes(), header.value);
                }
            }
        }
    }

    /// Feeds `input` one byte at a time, as a connection would on 1-byte reads. Returns how the parser handles a
    /// partial head and the head length it reports at the end.
    pub fn feed(self, input: &[u8]) -> (&'static str, usize) {
        match self {
            // Resumable: each call continues where the previous one stopped.
            Self::Blazehttp | Self::BlazehttpFields => {
                let mut parser = ResponseParser::new(Method::Get);
                for end in 1..input.len() {
                    let status = parser.parse(black_box(&input[..end]));
                    assert!(
                        matches!(status, Ok(ParseStatus::Partial)),
                        "partial at {end}: {status:?}"
                    );
                }
                match parser.parse(input) {
                    Ok(ParseStatus::Complete(head)) => ("incremental", head.head_len),
                    other => panic!("feed did not complete: {other:?}"),
                }
            }
            // Not resumable: every read re-parses the buffer from its first byte.
            Self::Httparse => {
                let mut head_len = 0;
                for end in 1..=input.len() {
                    let mut storage = [const { MaybeUninit::uninit() }; FIELD_SLOTS];
                    let (_, len) = httparse_parse(black_box(&input[..end]), &mut storage)
                        .expect("valid prefix");
                    head_len = len;
                }
                ("restart", head_len)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{NO_FIELDS, split_fields};

    #[test]
    fn split_fields_mirrors_httparse() {
        let head = b"HTTP/1.1 200 OK\r\na: 1\r\nb:\t two  \r\nc:\r\n\r\n";
        let mut spans = NO_FIELDS;
        let count = split_fields(head, 17..head.len() - 2, &mut spans);
        let got: Vec<(&[u8], &[u8])> = spans[..count]
            .iter()
            .map(|s| (s.name(head), s.value(head)))
            .collect();
        assert_eq!(got, [(&b"a"[..], &b"1"[..]), (b"b", b"two"), (b"c", b"")]);
        assert_eq!(split_fields(head, 17..17, &mut spans), 0);
    }
}
