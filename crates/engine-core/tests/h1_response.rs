//! `ResponseParser` against the vector corpus (`tests/vectors`, suite `engine-core:h1_response`) and the unit cases of
//! `docs/notes/response-parser.md` §9.6.

mod support {
    pub mod fixtures;
    pub mod response_vectors;
    pub mod vectors;
}

use engine_core::generated::spec::{
    EngineDetail, ErrorCode, H1_LIMITS, Method, NetworkDetail, ProtocolDetail,
};
use engine_core::h1::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
use support::fixtures;
use support::response_vectors::{ResponseCase, ResponseExpect};
use support::vectors::load_suite;

const SUITE: &str = "engine-core:h1_response";
const MALFORMED: ErrorCode = ErrorCode::Protocol(ProtocolDetail::MalformedResponse);
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);

/// What a connection delivers for one exchange when the server sends `input` and closes.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Ok {
        head: ResponseHead,
        headers: Vec<(Vec<u8>, Vec<u8>)>,
        body: Vec<u8>,
    },
    /// A chunked body: decoding arrives in P1a (#7). An explicit outcome, so a corpus case that needs it fails its
    /// comparison by name instead of aborting the whole test.
    ChunkedBody(ResponseHead),
    Error(ErrorCode),
}

impl Outcome {
    fn to_expect(&self) -> ResponseExpect {
        match self {
            Self::Ok {
                head,
                headers,
                body,
            } => ResponseExpect::Ok {
                status: head.status,
                headers: headers.clone(),
                body: body.clone(),
                keep_alive: head.keep_alive,
            },
            Self::ChunkedBody(_) => ResponseExpect::Error("chunked body (P1a, #7)".to_owned()),
            Self::Error(e) => ResponseExpect::Error(e.to_string()),
        }
    }
}

fn method_of(case: &ResponseCase) -> Method {
    (0..)
        .map_while(Method::from_wire)
        .find(|m| m.as_str() == case.method)
        .unwrap_or_else(|| panic!("{}: unknown method", case.name))
}

/// The final head's field lines as `(name, value without surrounding OWS)`, as the JS side reads them.
fn fields(block: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let block = block.strip_suffix(b"\r\n").unwrap_or(block);
    if block.is_empty() {
        return Vec::new();
    }
    block
        .split(|&b| b == b'\n')
        .map(|line| {
            let line = line.strip_suffix(b"\r").unwrap_or(line);
            let colon = line.iter().position(|&b| b == b':').expect("a colon");
            let value = &line[colon + 1..];
            let trimmed =
                value
                    .iter()
                    .position(|&b| b != b' ' && b != b'\t')
                    .map_or(&[][..], |start| {
                        let end = value.iter().rposition(|&b| b != b' ' && b != b'\t');
                        &value[start..=end.expect("a non-OWS byte")]
                    });
            (line[..colon].to_vec(), trimmed.to_vec())
        })
        .collect()
}

/// Body bytes after a completed head; the connection owns body reading, so `Content-Length` slicing lives here.
fn finish(head: ResponseHead, message: &[u8]) -> Outcome {
    let rest = &message[head.head_len..];
    let body = match head.framing {
        Framing::Empty => Vec::new(),
        Framing::CloseDelimited => rest.to_vec(),
        Framing::ContentLength(n) => match usize::try_from(n).ok().and_then(|n| rest.get(..n)) {
            Some(body) => body.to_vec(),
            None => return Outcome::Error(ErrorCode::Protocol(ProtocolDetail::IncompleteBody)),
        },
        Framing::Chunked => return Outcome::ChunkedBody(head),
    };
    Outcome::Ok {
        headers: fields(&message[head.headers.clone()]),
        head,
        body,
    }
}

/// Drives one exchange: `reads` are the input lengths available after each read (increasing), then EOF.
/// After `Interim { consumed }` the bytes are dropped and parsing continues on the same read.
fn exchange(input: &[u8], method: Method, reads: impl IntoIterator<Item = usize>) -> Outcome {
    let mut parser = ResponseParser::new(method);
    let mut start = 0;
    for available in reads {
        loop {
            match parser.parse(&input[start..available]) {
                Ok(ParseStatus::Partial) => break,
                Ok(ParseStatus::Interim { consumed }) => start += consumed,
                Ok(ParseStatus::Complete(head)) => return finish(head, &input[start..]),
                Err(e) => return Outcome::Error(e),
            }
        }
    }
    // EOF before the final head (design §5.2): only an exchange that received no byte at all is a network error.
    Outcome::Error(if input.is_empty() {
        ErrorCode::Network(NetworkDetail::ClosedBeforeResponse)
    } else {
        MALFORMED
    })
}

fn parse_whole(case: &ResponseCase) -> Outcome {
    exchange(&case.input, method_of(case), [case.input.len()])
}

fn parse_fed_in_two(case: &ResponseCase, split: usize) -> Outcome {
    exchange(&case.input, method_of(case), [split, case.input.len()])
}

fn parse_byte_by_byte(case: &ResponseCase) -> Outcome {
    exchange(&case.input, method_of(case), 1..=case.input.len())
}

#[test]
fn all_h1_response_vectors() {
    for (file, case) in load_suite::<ResponseCase>(SUITE) {
        assert_eq!(
            parse_whole(&case).to_expect(),
            case.expect,
            "{file}/{}",
            case.name
        );
    }
}

#[test]
fn every_vector_is_split_invariant() {
    for (file, case) in load_suite::<ResponseCase>(SUITE) {
        let whole = parse_whole(&case);
        for split in 1..case.input.len() {
            assert_eq!(
                parse_fed_in_two(&case, split),
                whole,
                "{file}/{} split {split}",
                case.name
            );
        }
        assert_eq!(
            parse_byte_by_byte(&case),
            whole,
            "{file}/{} 1-byte reads",
            case.name
        );
    }
}

/// Tripwire: `ErrorCode` carries no bytes today, so this holds by construction; it fails the day an error variant
/// gains a payload that could echo peer bytes (design §5.2 "Errors never echo raw header bytes").
#[test]
fn every_error_is_a_payload_free_code() {
    let mut errors = 0;
    for (file, case) in load_suite::<ResponseCase>(SUITE) {
        if let Outcome::Error(e) = parse_whole(&case) {
            errors += 1;
            assert!(
                ErrorCode::ALL_SAMPLES.contains(&e),
                "{file}/{}: {e:?}",
                case.name
            );
        }
    }
    assert_ne!(errors, 0);
}

fn complete(parser: &mut ResponseParser, buf: &[u8]) -> ResponseHead {
    match parser.parse(buf) {
        Ok(ParseStatus::Complete(head)) => head,
        other => panic!("expected a complete head, got {other:?}"),
    }
}

#[test]
fn fixture_heads_parse_to_their_facts() {
    for (name, fields, keep_alive) in [
        ("1k.http", 3, true),
        ("1k-h20.http", 20, true),
        ("synthetic-h20.http", 20, true),
    ] {
        let head_bytes = fixtures::h1_head(name);
        let head = complete(&mut ResponseParser::new(Method::Get), &head_bytes);
        assert_eq!(head.status, 200, "{name}");
        assert_eq!(head.version, HttpVersion::Http11, "{name}");
        assert_eq!(head.head_len, head_bytes.len(), "{name}");
        assert_eq!(head.framing, Framing::ContentLength(847), "{name}");
        assert_eq!(head.keep_alive, keep_alive, "{name}");
        assert_eq!(head.headers, 17..head_bytes.len() - 2, "{name}");
        assert_eq!(fields_of(&head_bytes, &head), fields, "{name}");
        let byte_by_byte = exchange(&head_bytes, Method::Get, 1..=head_bytes.len());
        assert_eq!(
            byte_by_byte,
            exchange(&head_bytes, Method::Get, [head_bytes.len()]),
            "{name}"
        );
    }
}

fn fields_of(head_bytes: &[u8], head: &ResponseHead) -> usize {
    fields(&head_bytes[head.headers.clone()]).len()
}

#[test]
fn head_without_fields_has_an_empty_header_range() {
    let head = complete(
        &mut ResponseParser::new(Method::Get),
        b"HTTP/1.0 404 Not Found\r\n\r\n",
    );
    assert_eq!(head.headers, 24..24);
    assert_eq!(head.version, HttpVersion::Http10);
    assert_eq!(head.framing, Framing::CloseDelimited);
    assert!(!head.keep_alive);
}

#[test]
fn te_on_get_framing() {
    let input = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    let Outcome::ChunkedBody(head) = exchange(input, Method::Get, [input.len()]) else {
        panic!("a GET with Transfer-Encoding: chunked has a chunked body");
    };
    assert_eq!(head.framing, Framing::Chunked);
    assert!(head.keep_alive);
}

#[test]
fn bodiless_statuses_and_head_have_empty_framing() {
    for (method, input, keep_alive) in [
        (
            Method::Get,
            &b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n"[..],
            true,
        ),
        (
            Method::Get,
            b"HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\n\r\n",
            true,
        ),
        (
            Method::Get,
            b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\n",
            false,
        ),
        (
            Method::Head,
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\n",
            true,
        ),
        (
            Method::Head,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n",
            true,
        ),
        (Method::Head, b"HTTP/1.1 200 OK\r\n\r\n", true),
    ] {
        let head = complete(&mut ResponseParser::new(method), input);
        assert_eq!(head.framing, Framing::Empty, "{method:?} {input:?}");
        assert_eq!(head.keep_alive, keep_alive, "{method:?} {input:?}");
    }
}

#[test]
fn parse_after_complete_is_a_contract_violation() {
    let input = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
    let mut parser = ResponseParser::new(Method::Get);
    complete(&mut parser, input);
    assert_eq!(parser.parse(input), Err(INTERNAL));
}

#[test]
fn parse_after_an_error_is_a_contract_violation() {
    let mut parser = ResponseParser::new(Method::Get);
    assert_eq!(parser.parse(b"HTTP/1.1 200 OK\nX"), Err(MALFORMED));
    assert_eq!(
        parser.parse(b"HTTP/1.1 200 OK\r\n\r\n"),
        Err(INTERNAL),
        "the Failed phase is terminal"
    );
}

#[test]
fn a_buffer_shorter_than_the_examined_prefix_is_a_contract_violation() {
    let input = b"HTTP/1.1 200 OK\r\nFoo";
    let mut parser = ResponseParser::new(Method::Get);
    assert_eq!(parser.parse(input), Ok(ParseStatus::Partial));
    assert_eq!(parser.parse(&input[..15]), Err(INTERNAL));
}

#[test]
fn a_buffer_shorter_than_the_status_prefix_after_it_was_examined_is_a_contract_violation() {
    let mut parser = ResponseParser::new(Method::Get);
    assert_eq!(parser.parse(b"HTTP/1.1 200"), Ok(ParseStatus::Partial));
    assert_eq!(parser.parse(b"HTTP/"), Err(INTERNAL));
}

#[test]
fn a_shorter_buffer_during_the_status_prefix_is_re_validated() {
    let mut parser = ResponseParser::new(Method::Get);
    assert_eq!(parser.parse(b"HTTP/1.1 2"), Ok(ParseStatus::Partial));
    assert_eq!(parser.parse(b"HTTP/"), Ok(ParseStatus::Partial));
    complete(&mut parser, b"HTTP/1.1 200 OK\r\n\r\n");
}

#[test]
fn reset_after_complete_parses_the_next_exchange() {
    let mut parser = ResponseParser::new(Method::Head);
    let first = complete(&mut parser, b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n");
    assert_eq!(first.framing, Framing::Empty);
    parser.reset(Method::Get);
    let second = complete(&mut parser, b"HTTP/1.1 200 OK\r\nContent-Length: 9\r\n\r\n");
    assert_eq!(second.framing, Framing::ContentLength(9));
}

#[test]
fn reset_clears_interim_accounting_and_sticky_close() {
    let mut parser = ResponseParser::new(Method::Get);
    let interim = b"HTTP/1.1 103 Early Hints\r\nConnection: close\r\n\r\n";
    assert_eq!(
        parser.parse(interim),
        Ok(ParseStatus::Interim {
            consumed: interim.len()
        })
    );
    parser.reset(Method::Get);
    let head = complete(&mut parser, b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
    assert!(head.keep_alive);
}

#[test]
fn garbage_fails_at_its_first_byte_without_waiting_for_a_line_end() {
    let mut parser = ResponseParser::new(Method::Get);
    assert_eq!(parser.parse(b"\x16\x03\x03\x00\x02"), Err(MALFORMED));
}

#[test]
fn a_head_of_64_kib_fed_one_byte_at_a_time_completes() {
    let mut input = b"HTTP/1.1 200 OK\r\n".to_vec();
    let line = format!("x-fill: {}\r\n", "a".repeat(990));
    while input.len() + line.len() + 2 <= H1_LIMITS.max_head_bytes {
        input.extend_from_slice(line.as_bytes());
    }
    let pad = H1_LIMITS.max_head_bytes - input.len() - 2 - "x-pad: \r\n".len();
    input.extend_from_slice(format!("x-pad: {}\r\n\r\n", "b".repeat(pad)).as_bytes());
    assert_eq!(input.len(), H1_LIMITS.max_head_bytes);
    let mut parser = ResponseParser::new(Method::Get);
    for end in 1..input.len() {
        assert_eq!(parser.parse(&input[..end]), Ok(ParseStatus::Partial));
    }
    assert_eq!(complete(&mut parser, &input).head_len, input.len());
}
