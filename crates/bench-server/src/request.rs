//! Request boundaries: just enough of a request head to answer it (design note §5). Nothing else is parsed.

const HEAD_END: &[u8] = b"\r\n\r\n";
const CRLF: &[u8] = b"\r\n";
/// Empty lines accepted before a request line (RFC 9112 §2.2 asks servers to ignore at least one).
const MAX_LEADING_EMPTY_LINES: usize = 8;

/// A complete request head at the start of the scanned bytes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request<'a> {
    /// The request target: the bytes between the first two spaces of the request line.
    pub(crate) target: &'a [u8],
    /// `HEAD`: answer with the response head only.
    pub(crate) head_only: bool,
    /// Head bytes including leading empty lines and the blank line.
    pub(crate) head_len: usize,
    /// Request body bytes that follow the head (from `content-length`).
    pub(crate) body_len: u64,
    /// The client asked to close after this response (`Connection: close`, or HTTP/1.0 without keep-alive).
    pub(crate) close: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scan<'a> {
    Complete(Request<'a>),
    /// No blank line yet; the next scan of the same (grown) bytes resumes at `resume`.
    Partial {
        resume: usize,
    },
    /// A head this fixture refuses (answered with 400): see [`scan`].
    Invalid,
}

/// The request line's parts this fixture uses.
struct RequestLine<'a> {
    target: &'a [u8],
    head_only: bool,
    http10: bool,
}

/// What the field lines say about framing and persistence.
#[derive(Default)]
struct Fields {
    body_len: u64,
    close: bool,
    keep_alive: bool,
}

/// Scans `input` for one request head, starting the blank-line search at `resume` so a peer that sends one byte at a
/// time costs O(n) in total, not O(n²).
///
/// Refused: more than 8 leading empty lines, a request line without a target or with a version other than HTTP/1.0
/// or HTTP/1.1, a field line without a colon, whitespace in a field name, obs-fold, `transfer-encoding`, and a
/// missing, repeated or non-decimal `content-length` value.
pub(crate) fn scan(input: &[u8], resume: usize) -> Scan<'_> {
    let Some(skipped) = leading_empty_lines(input) else {
        return Scan::Invalid;
    };
    let Some(end) = find_head_end(input, resume.max(skipped)) else {
        let resume = input.len().saturating_sub(HEAD_END.len() - 1).max(skipped);
        return Scan::Partial { resume };
    };
    let mut lines = input[skipped..end]
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    match (lines.next().and_then(request_line), fields(lines)) {
        (Some(line), Some(fields)) => Scan::Complete(Request {
            target: line.target,
            head_only: line.head_only,
            head_len: end + HEAD_END.len(),
            body_len: fields.body_len,
            close: fields.close || (line.http10 && !fields.keep_alive),
        }),
        _ => Scan::Invalid,
    }
}

/// The offset of the blank line (`\r\n\r\n`) at or after `from`.
pub(crate) fn find_head_end(input: &[u8], from: usize) -> Option<usize> {
    let tail = input.get(from..)?;
    tail.windows(HEAD_END.len())
        .position(|w| w == HEAD_END)
        .map(|at| from + at)
}

/// Bytes of empty lines before the request line; `None` past the limit.
fn leading_empty_lines(input: &[u8]) -> Option<usize> {
    let (pairs, _) = input.as_chunks::<2>();
    let lines = pairs.iter().take_while(|pair| pair[..] == *CRLF).count();
    (lines <= MAX_LEADING_EMPTY_LINES).then_some(lines * CRLF.len())
}

fn request_line(line: &[u8]) -> Option<RequestLine<'_>> {
    let mut parts = line.splitn(3, |&b| b == b' ');
    let (method, target, version) = (parts.next()?, parts.next()?, parts.next()?);
    let http10 = match version {
        b"HTTP/1.1" => false,
        b"HTTP/1.0" => true,
        _ => return None,
    };
    Some(RequestLine {
        target,
        head_only: method == b"HEAD",
        http10,
    })
}

/// Framing and persistence from the field lines; `None` when the head is refused.
fn fields<'a>(lines: impl Iterator<Item = &'a [u8]>) -> Option<Fields> {
    let mut fields = Fields::default();
    let mut content_length = None;
    for line in lines {
        let (name, value) = field_line(line)?;
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return None;
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            content_length = Some(decimal(value).filter(|_| content_length.is_none())?);
        }
        if name.eq_ignore_ascii_case(b"connection") {
            fields.close |= has_token(value, b"close");
            fields.keep_alive |= has_token(value, b"keep-alive");
        }
    }
    fields.body_len = content_length.unwrap_or(0);
    Some(fields)
}

/// Name and trimmed value of one field line; `None` for obs-fold, no colon or whitespace in the name.
fn field_line(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let colon = line.iter().position(|&b| b == b':')?;
    let (name, value) = (&line[..colon], &line[colon + 1..]);
    let whitespace = |b: &u8| matches!(b, b' ' | b'\t');
    (!name.is_empty() && !name.iter().any(whitespace)).then_some((name, value.trim_ascii()))
}

fn decimal(digits: &[u8]) -> Option<u64> {
    if digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(digits).ok()?.parse().ok()
}

fn has_token(list: &[u8], token: &[u8]) -> bool {
    list.split(|&b| b == b',')
        .any(|item| item.trim_ascii().eq_ignore_ascii_case(token))
}

#[cfg(test)]
mod tests {
    use super::{Request, Scan, scan};

    /// Heads the scan refuses, one per rule in [`scan`]'s documentation.
    const INVALID: &[&[u8]] = &[
        b"GET /1k\r\n\r\n",
        b"GET /1k HTTP/2.0\r\n\r\n",
        b"\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\n\r\nGET / HTTP/1.1\r\n\r\n",
        b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n",
        b"POST / HTTP/1.1\r\ncontent-length: +1\r\n\r\n",
        b"POST / HTTP/1.1\r\ncontent-length:\r\n\r\n",
        b"POST / HTTP/1.1\r\ncontent-length: 1\r\ncontent-length: 1\r\n\r\n",
        b"POST / HTTP/1.1\r\ncontent-length: 99999999999999999999\r\n\r\n",
        b"POST / HTTP/1.1\r\nno-colon\r\n\r\n",
        b"POST / HTTP/1.1\r\n: empty-name\r\n\r\n",
        b"POST / HTTP/1.1\r\ncontent-length : 1\r\n\r\n",
        b"POST / HTTP/1.1\r\nhost: a\r\n folded\r\n\r\n",
        b"POST / HTTP/1.1\r\nhost: a\r\n\tfolded\r\n\r\n",
    ];

    fn request(target: &[u8], head_len: usize) -> Request<'_> {
        Request {
            target,
            head_only: false,
            head_len,
            body_len: 0,
            close: false,
        }
    }

    #[test]
    fn takes_the_target_and_head_length() {
        let input = b"GET /1k HTTP/1.1\r\nhost: a\r\n\r\nGET";
        assert_eq!(scan(input, 0), Scan::Complete(request(b"/1k", 29)));
    }

    #[test]
    fn reads_content_length_case_insensitively_with_ows() {
        let input = b"POST /x HTTP/1.1\r\nContent-LENGTH: \t42 \r\n\r\n";
        let expected = Request {
            body_len: 42,
            ..request(b"/x", input.len())
        };
        assert_eq!(scan(input, 0), Scan::Complete(expected));
    }

    #[test]
    fn partial_head_resumes_before_a_split_terminator() {
        let input = b"GET /1k HTTP/1.1\r\n\r";
        assert_eq!(
            scan(input, 0),
            Scan::Partial {
                resume: input.len() - 3
            }
        );
        let grown = b"GET /1k HTTP/1.1\r\n\r\n";
        assert_eq!(
            scan(grown, input.len() - 3),
            Scan::Complete(request(b"/1k", grown.len()))
        );
    }

    #[test]
    fn skips_leading_empty_lines() {
        let input = b"\r\n\r\nHEAD /1k HTTP/1.1\r\n\r\n";
        let expected = Request {
            head_only: true,
            ..request(b"/1k", input.len())
        };
        assert_eq!(scan(input, 0), Scan::Complete(expected));
        assert_eq!(scan(b"\r\n\r\n\r", 0), Scan::Partial { resume: 4 });
    }

    #[test]
    fn close_follows_the_connection_field_and_version() {
        let cases: [(&[u8], bool); 5] = [
            (
                b"GET / HTTP/1.1\r\nconnection: Keep-Alive, Close\r\n\r\n",
                true,
            ),
            (b"GET / HTTP/1.1\r\nconnection: keep-alive\r\n\r\n", false),
            (b"GET / HTTP/1.0\r\n\r\n", true),
            (b"GET / HTTP/1.0\r\nConnection: keep-alive\r\n\r\n", false),
            (
                b"GET / HTTP/1.0\r\nconnection: keep-alive, close\r\n\r\n",
                true,
            ),
        ];
        for (input, close) in cases {
            let Scan::Complete(request) = scan(input, 0) else {
                panic!("complete")
            };
            assert_eq!(request.close, close, "{}", String::from_utf8_lossy(input));
        }
    }

    #[test]
    fn refuses_framing_it_does_not_handle() {
        for &input in INVALID {
            assert_eq!(
                scan(input, 0),
                Scan::Invalid,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }
}
