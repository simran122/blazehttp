//! Request boundaries: just enough of a request head to answer it (design note §5). Nothing else is parsed.

use crate::cells::Refusal;

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
    /// An HTTP/1.0 request (it cannot receive a chunked response, RFC 9112 §6.1).
    pub(crate) http10: bool,
    /// The head parsed, but the request is refused (an unsupported HTTP major version): answer with this refusal
    /// once the declared body is read, then close.
    pub(crate) refusal: Option<Refusal>,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scan<'a> {
    Complete(Request<'a>),
    /// No blank line yet; the next scan of the same (grown) bytes resumes at `resume`.
    Partial {
        resume: usize,
    },
    /// A head this fixture refuses, and the error response it gets: see [`scan`].
    Refused(Refusal),
}

/// The request line's parts this fixture uses.
struct RequestLine<'a> {
    target: &'a [u8],
    head_only: bool,
    http10: bool,
}

impl RequestLine<'_> {
    /// Stands in for a request line whose version is refused: no target is looked up.
    const REFUSED: RequestLine<'static> = RequestLine {
        target: b"",
        head_only: false,
        http10: false,
    };
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
/// HTTP/1.x with a minor version above 1 is treated as HTTP/1.1 (RFC 9110 §2.5); another major version gets 505.
/// Refused with 400: more than 8 leading empty lines, a request line with an empty method or target or a malformed
/// version or a bare CR, a field line without a colon, whitespace or CR in a field name or an empty name, obs-fold,
/// `transfer-encoding`, and a missing, repeated or non-decimal `content-length` value.
pub(crate) fn scan(input: &[u8], resume: usize) -> Scan<'_> {
    let Some(skipped) = leading_empty_lines(input) else {
        return Scan::Refused(Refusal::BadRequest);
    };
    let Some(end) = find_head_end(input, resume.max(skipped)) else {
        let resume = input.len().saturating_sub(HEAD_END.len() - 1).max(skipped);
        return Scan::Partial { resume };
    };
    let mut lines = input[skipped..end]
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let line = request_line(lines.next().unwrap_or_default());
    let Some(fields) = fields(lines) else {
        return Scan::Refused(Refusal::BadRequest);
    };
    let (line, refusal) = match line {
        Ok(line) => (line, None),
        Err(Refusal::VersionNotSupported) => {
            (RequestLine::REFUSED, Some(Refusal::VersionNotSupported))
        }
        Err(kind) => return Scan::Refused(kind),
    };
    Scan::Complete(Request {
        target: line.target,
        head_only: line.head_only,
        head_len: end + HEAD_END.len(),
        body_len: fields.body_len,
        close: refusal.is_some() || fields.close || (line.http10 && !fields.keep_alive),
        http10: line.http10,
        refusal,
    })
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

fn request_line(line: &[u8]) -> Result<RequestLine<'_>, Refusal> {
    let mut parts = line.splitn(3, |&b| b == b' ');
    let (Some(method), Some(target), Some(version)) = (parts.next(), parts.next(), parts.next())
    else {
        return Err(Refusal::BadRequest);
    };
    // A CR inside the line is a bare CR (the line's own CRLF is already removed): RFC 9112 §2.2 says to refuse it.
    if method.is_empty() || target.is_empty() || line.contains(&b'\r') {
        return Err(Refusal::BadRequest);
    }
    Ok(RequestLine {
        target,
        head_only: method == b"HEAD",
        http10: http10(version)?,
    })
}

/// `true` for HTTP/1.0, `false` for HTTP/1.1 and higher 1.x minors; 505 for another major, 400 when malformed.
fn http10(version: &[u8]) -> Result<bool, Refusal> {
    match version {
        b"HTTP/1.0" => Ok(true),
        [b'H', b'T', b'T', b'P', b'/', b'1', b'.', minor] if minor.is_ascii_digit() => Ok(false),
        [b'H', b'T', b'T', b'P', b'/', major, b'.', minor]
            if major.is_ascii_digit() && minor.is_ascii_digit() =>
        {
            Err(Refusal::VersionNotSupported)
        }
        _ => Err(Refusal::BadRequest),
    }
}

/// Framing and persistence from the field lines; `None` when the head is refused.
fn fields<'a>(lines: impl Iterator<Item = &'a [u8]>) -> Option<Fields> {
    let mut fields = Fields::default();
    let mut content_length = None;
    for line in lines {
        let (name, value) = field_line(line)?;
        // TODO(#16): skip chunked request bodies for the G1 streaming-upload cell instead of refusing them.
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

/// Name and trimmed value of one field line; `None` for obs-fold, no colon, an empty name, or whitespace or CR in the
/// name.
fn field_line(line: &[u8]) -> Option<(&[u8], &[u8])> {
    let colon = line.iter().position(|&b| b == b':')?;
    let (name, value) = (&line[..colon], &line[colon + 1..]);
    let not_in_name = |b: &u8| matches!(b, b' ' | b'\t' | b'\r');
    (!name.is_empty() && !name.iter().any(not_in_name)).then_some((name, value.trim_ascii()))
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
    use crate::cells::Refusal;

    /// Heads the scan refuses, one per rule in [`scan`]'s documentation.
    const INVALID: &[&[u8]] = &[
        b"GET /1k\r\n\r\n",
        b"GET /1k HTTP/9\r\n\r\n",
        b"GET /1k HTTP/1.10\r\n\r\n",
        b"GET /1k http/1.1\r\n\r\n",
        b"GET /1k HTTP/1.x\r\n\r\n",
        b"GET /1k HTTP/a.1\r\n\r\n",
        b"GET /1k HTTP/2.x\r\n\r\n",
        b"GET /1k\r HTTP/1.1\r\n\r\n",
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
        b"GET  HTTP/1.1\r\n\r\n",
        b" /1k HTTP/1.1\r\n\r\n",
        b"GET /1k HTTP/1.1\r\ncontent-length\r: 5\r\n\r\n",
    ];

    fn request(target: &[u8], head_len: usize) -> Request<'_> {
        Request {
            target,
            head_only: false,
            head_len,
            body_len: 0,
            close: false,
            http10: false,
            refusal: None,
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
                Scan::Refused(Refusal::BadRequest),
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }

    #[test]
    fn versions_follow_rfc_9110() {
        let get = |version: &str| format!("GET /1k {version}\r\n\r\n").into_bytes();
        let http10 = get("HTTP/1.0");
        let Scan::Complete(parsed) = scan(&http10, 0) else {
            panic!("HTTP/1.0 is complete")
        };
        assert!(parsed.http10 && parsed.close);
        let http12 = get("HTTP/1.2");
        assert_eq!(
            scan(&http12, 0),
            Scan::Complete(request(b"/1k", http12.len()))
        );
        for version in ["HTTP/2.0", "HTTP/3.0"] {
            let input = get(version);
            let Scan::Complete(refused) = scan(&input, 0) else {
                panic!("{version}: the head parses")
            };
            assert_eq!(
                refused.refusal,
                Some(Refusal::VersionNotSupported),
                "{version}"
            );
            assert!(refused.close, "{version}");
        }
    }
}
