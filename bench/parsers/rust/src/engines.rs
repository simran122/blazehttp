//! The two Rust parsers, each through its fastest idiomatic API, doing the same work per head: validate the head,
//! find its end, and derive the framing facts (`Content-Length`, chunked, `Connection: close`).

use std::mem::MaybeUninit;

use engine_core::generated::spec::Method;
use engine_core::h1::{Framing, ParseStatus, ResponseHead, ResponseParser};

/// httparse needs a caller-provided header array; 100 covers the 99-field feed input (b).
const HTTPARSE_HEADERS: usize = 100;

#[derive(Clone, Copy)]
pub enum Engine {
    Blazehttp,
    Httparse,
}

/// What `check` prints for one head (compared across all languages by the runner's differential gate).
pub struct Description {
    pub status: u16,
    pub head_len: usize,
    pub content_length: Option<u64>,
    pub fields: Vec<(Vec<u8>, Vec<u8>)>,
}

/// The facts both parsers produce on the timed path, folded into one number so the work cannot be optimised away.
fn fold(status: u16, head_len: usize, framing: Framing, keep_alive: bool) -> u64 {
    let length = match framing {
        Framing::ContentLength(n) => n,
        Framing::Chunked => 1 << 40,
        Framing::CloseDelimited | Framing::Empty => 0,
    };
    u64::from(status) + head_len as u64 + length + u64::from(keep_alive)
}

fn ours(head: &[u8]) -> ResponseHead {
    match ResponseParser::new(Method::Get).parse(head) {
        Ok(ParseStatus::Complete(parsed)) => parsed,
        other => panic!("blazehttp did not parse the head: {other:?}"),
    }
}

/// Field lines of a validated head: our parser returns offsets only, so names and values are split here (outside the
/// timed path), with the value's surrounding OWS removed (RFC 9112 §5).
fn ours_fields(head: &[u8], parsed: &ResponseHead) -> Vec<(Vec<u8>, Vec<u8>)> {
    head[parsed.headers.clone()]
        .split(|&b| b == b'\n')
        .filter_map(|line| line.strip_suffix(b"\r"))
        .filter(|line| !line.is_empty())
        .map(|line| {
            let colon = line
                .iter()
                .position(|&b| b == b':')
                .expect("validated field line");
            // A validated value holds no CR, LF or form feed, so ASCII whitespace here is SP/HTAB only.
            (
                line[..colon].to_vec(),
                line[colon + 1..].trim_ascii().to_vec(),
            )
        })
        .collect()
}

fn httparse_parse<'h, 'b>(
    head: &'b [u8],
    storage: &'h mut [MaybeUninit<httparse::Header<'b>>; HTTPARSE_HEADERS],
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
fn httparse_facts(response: &httparse::Response<'_, '_>) -> (Framing, bool) {
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
    let framing = match (chunked, length) {
        (true, _) => Framing::Chunked,
        (false, Some(n)) => Framing::ContentLength(n),
        (false, None) => Framing::CloseDelimited,
    };
    (framing, !close && framing != Framing::CloseDelimited)
}

impl Engine {
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "blazehttp" => Some(Self::Blazehttp),
            "httparse" => Some(Self::Httparse),
            _ => None,
        }
    }

    /// The timed path: one complete head in, the folded facts out.
    pub fn parse(self, head: &[u8]) -> u64 {
        match self {
            Self::Blazehttp => {
                let parsed = ours(head);
                fold(
                    parsed.status,
                    parsed.head_len,
                    parsed.framing,
                    parsed.keep_alive,
                )
            }
            Self::Httparse => {
                let mut storage = [const { MaybeUninit::uninit() }; HTTPARSE_HEADERS];
                let (response, head_len) =
                    httparse_parse(head, &mut storage).expect("httparse parses the head");
                let (framing, keep_alive) = httparse_facts(&response);
                fold(response.code.unwrap_or(0), head_len, framing, keep_alive)
            }
        }
    }

    pub fn describe(self, head: &[u8]) -> Description {
        match self {
            Self::Blazehttp => {
                let parsed = ours(head);
                Description {
                    status: parsed.status,
                    head_len: parsed.head_len,
                    content_length: match parsed.framing {
                        Framing::ContentLength(n) => Some(n),
                        _ => None,
                    },
                    fields: ours_fields(head, &parsed),
                }
            }
            Self::Httparse => {
                let mut storage = [const { MaybeUninit::uninit() }; HTTPARSE_HEADERS];
                let (response, head_len) =
                    httparse_parse(head, &mut storage).expect("httparse parses the head");
                let content_length = match httparse_facts(&response).0 {
                    Framing::ContentLength(n) => Some(n),
                    _ => None,
                };
                Description {
                    status: response.code.unwrap_or(0),
                    head_len,
                    content_length,
                    fields: response
                        .headers
                        .iter()
                        .map(|h| (h.name.as_bytes().to_vec(), h.value.to_vec()))
                        .collect(),
                }
            }
        }
    }

    /// Feeds `input` one byte at a time, as a connection would on 1-byte reads. Returns how the parser handles a
    /// partial head and the head length it reports at the end.
    pub fn feed(self, input: &[u8]) -> (&'static str, usize) {
        match self {
            // Resumable: each call continues where the previous one stopped.
            Self::Blazehttp => {
                let mut parser = ResponseParser::new(Method::Get);
                for end in 1..input.len() {
                    let status = parser.parse(std::hint::black_box(&input[..end]));
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
                    let mut storage = [const { MaybeUninit::uninit() }; HTTPARSE_HEADERS];
                    let (_, len) =
                        httparse_parse(std::hint::black_box(&input[..end]), &mut storage)
                            .expect("valid prefix");
                    head_len = len;
                }
                ("restart", head_len)
            }
        }
    }
}
