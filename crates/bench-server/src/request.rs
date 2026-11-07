//! Request boundaries: just enough of a request head to answer it (design note §5). Nothing else is parsed.

const HEAD_END: &[u8] = b"\r\n\r\n";

/// A complete request head at the start of the scanned bytes.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Request<'a> {
    /// The request target: the bytes between the first two spaces of the request line.
    pub(crate) target: &'a [u8],
    /// Head bytes including the blank line.
    pub(crate) head_len: usize,
    /// Request body bytes that follow the head (from `content-length`).
    pub(crate) body_len: u64,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Scan<'a> {
    Complete(Request<'a>),
    /// No blank line yet; the next scan of the same (grown) bytes resumes at `resume`.
    Partial {
        resume: usize,
    },
    /// A head this fixture refuses to answer: no target, a bad `content-length`, or a `transfer-encoding`.
    Refused,
}

/// Scans `input` for one request head, starting the blank-line search at `resume` so a peer that sends one byte at a
/// time costs O(n) in total, not O(n²).
pub(crate) fn scan(input: &[u8], resume: usize) -> Scan<'_> {
    let Some(end) = find_head_end(input, resume) else {
        return Scan::Partial {
            resume: input.len().saturating_sub(HEAD_END.len() - 1),
        };
    };
    let head = &input[..end];
    let mut lines = head
        .split(|&b| b == b'\n')
        .map(|line| line.strip_suffix(b"\r").unwrap_or(line));
    let target = lines.next().and_then(target);
    match (target, body_len(lines)) {
        (Some(target), Some(body_len)) => Scan::Complete(Request {
            target,
            head_len: end + HEAD_END.len(),
            body_len,
        }),
        _ => Scan::Refused,
    }
}

/// The offset of the blank line (`\r\n\r\n`) at or after `from`.
pub(crate) fn find_head_end(input: &[u8], from: usize) -> Option<usize> {
    let tail = input.get(from..)?;
    tail.windows(HEAD_END.len())
        .position(|w| w == HEAD_END)
        .map(|at| from + at)
}

fn target(request_line: &[u8]) -> Option<&[u8]> {
    let mut parts = request_line.splitn(3, |&b| b == b' ');
    let (_method, target, _version) = (parts.next()?, parts.next()?, parts.next()?);
    Some(target)
}

/// Body length from the field lines: `Some(0)` without `content-length`, `None` when the framing is refused.
fn body_len<'a>(fields: impl Iterator<Item = &'a [u8]>) -> Option<u64> {
    let mut len = None;
    for field in fields {
        let (name, value) = field.split_at(field.iter().position(|&b| b == b':')?);
        if name.eq_ignore_ascii_case(b"transfer-encoding") {
            return None;
        }
        if name.eq_ignore_ascii_case(b"content-length") {
            let digits = value[1..].trim_ascii();
            if len.is_some() || digits.is_empty() || !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            len = Some(std::str::from_utf8(digits).ok()?.parse().ok()?);
        }
    }
    Some(len.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::{Request, Scan, scan};

    fn complete(target: &[u8], head_len: usize, body_len: u64) -> Scan<'_> {
        Scan::Complete(Request {
            target,
            head_len,
            body_len,
        })
    }

    #[test]
    fn takes_the_target_and_head_length() {
        let input = b"GET /1k HTTP/1.1\r\nhost: a\r\n\r\nGET";
        assert_eq!(scan(input, 0), complete(b"/1k", 29, 0));
    }

    #[test]
    fn reads_content_length_case_insensitively_with_ows() {
        let input = b"POST /x HTTP/1.1\r\nContent-LENGTH: \t42 \r\n\r\n";
        assert_eq!(scan(input, 0), complete(b"/x", input.len(), 42));
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
            complete(b"/1k", grown.len(), 0)
        );
    }

    #[test]
    fn refuses_framing_it_does_not_handle() {
        for input in [
            &b"GET /1k\r\n\r\n"[..],
            b"POST / HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: +1\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length:\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: 1\r\ncontent-length: 1\r\n\r\n",
            b"POST / HTTP/1.1\r\ncontent-length: 99999999999999999999\r\n\r\n",
            b"POST / HTTP/1.1\r\nno-colon\r\n\r\n",
        ] {
            assert_eq!(
                scan(input, 0),
                Scan::Refused,
                "{}",
                String::from_utf8_lossy(input)
            );
        }
    }
}
