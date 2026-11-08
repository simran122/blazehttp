//! Strict, resumable HTTP/1.1 response head parser (design §5.1–5.2; decisions in `docs/notes/response-parser.md`
//! §6–7). One forward scan over the head: the status prefix byte by byte, then each line part as one span (`TCHAR` run
//! for a name, `FIELD_CONTENT` run for a reason or value) that must end at a CRLF. A partial line resumes where the
//! scan stopped, so the whole head costs O(head bytes) however it is split. No allocation, no header map: the framing
//! and `Connection` facts are folded in as each field line completes.

use core::ops::Range;

mod facts;

use facts::{Facts, StatusLine};

use super::grammar::{token_len, value_len};
use crate::generated::spec::{
    EngineDetail, ErrorCode, H1_LIMITS, Method, ProtocolDetail, TooLargeDetail,
};

const MALFORMED: ErrorCode = ErrorCode::Protocol(ProtocolDetail::MalformedResponse);
const SMUGGLING: ErrorCode = ErrorCode::Protocol(ProtocolDetail::SmugglingRisk);
const TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);
/// A caller broke the `parse` contract (or an offset invariant failed): never a peer's fault.
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);

/// `HTTP/1.x DDD`: version, SP and the three status digits.
const STATUS_PREFIX_LEN: usize = 12;
/// First reason byte after `HTTP/1.x DDD SP`.
const REASON_START: usize = 13;
const CRLF_LEN: usize = 2;

/// HTTP version of a response's status line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpVersion {
    Http10,
    Http11,
}

/// How the body after a final head is delimited (design §5.1 `FramingDecoder`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Framing {
    ContentLength(u64),
    Chunked,
    /// The body runs to EOF; the connection is never reused.
    CloseDelimited,
    /// No body: `HEAD`, 204 or 304.
    Empty,
}

/// A validated final (non-1xx) response head; offsets are relative to the message start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResponseHead {
    pub status: u16,
    pub version: HttpVersion,
    /// Bytes of the head including the blank line; the body starts here.
    pub head_len: usize,
    /// Every field line with its CRLF (`"name: value\r\n"*`), without the blank line; empty when there are none.
    pub headers: Range<usize>,
    pub framing: Framing,
    /// Whether the head and its framing allow reusing the connection (§5.1 adds connection-level conditions).
    pub keep_alive: bool,
}

/// Result of one [`ResponseParser::parse`] call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseStatus {
    Complete(ResponseHead),
    /// A 1xx head of `consumed` bytes was skipped: drop them and call again with the bytes after it.
    Interim {
        consumed: usize,
    },
    /// The head is not complete yet: call again with the same bytes plus more.
    Partial,
}

/// Where the scan resumes. Each resumable phase is one line part; `Done` and `Failed` are terminal until `reset`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    StatusPrefix,
    StatusReason,
    LineStart,
    Name,
    Value,
    Done,
    Failed,
}

/// Resumable parser for one exchange's response heads (interim heads, then the final head).
///
/// Invariants within a call: `line_start ≤ pos ≤ window.len()`; in phase `Value` also `line_start ≤ colon < pos`;
/// `interim_bytes ≤ H1_LIMITS.max_head_bytes`.
#[derive(Debug)]
pub struct ResponseParser {
    head_request: bool,
    phase: Phase,
    /// `[0, pos)` of the current message is fully examined.
    pos: usize,
    /// Start of the current line.
    line_start: usize,
    /// The current field line's `:` (meaningful in phase `Value`).
    colon: usize,
    // Per message, reset after every interim head.
    line: StatusLine,
    headers_start: usize,
    fields: usize,
    // Per exchange, reset only by `new`/`reset` (`facts` keeps a sticky `Connection: close` across interim heads).
    facts: Facts,
    interim_count: usize,
    interim_bytes: usize,
}

fn checked_add(a: usize, b: usize) -> Result<usize, ErrorCode> {
    a.checked_add(b).ok_or(INTERNAL)
}

/// Whether `b` may appear at offset `i` of the status prefix `HTTP/1.x DDD`: the single definition of that grammar.
fn status_prefix_byte(i: usize, b: u8) -> bool {
    match i {
        0..=6 => b"HTTP/1.".get(i) == Some(&b),
        7 => matches!(b, b'0' | b'1'),
        8 => b == b' ',
        _ => b.is_ascii_digit(),
    }
}

impl ResponseParser {
    const fn fresh(head_request: bool) -> Self {
        Self {
            head_request,
            phase: Phase::StatusPrefix,
            pos: 0,
            line_start: 0,
            colon: 0,
            line: StatusLine {
                status: 0,
                version: HttpVersion::Http11,
            },
            headers_start: 0,
            fields: 0,
            facts: Facts::NEW,
            interim_count: 0,
            interim_bytes: 0,
        }
    }

    /// A parser for the response to a `method` request (`HEAD` responses never have a body).
    #[must_use]
    pub fn new(method: Method) -> Self {
        Self::fresh(method == Method::Head)
    }

    /// Starts a new exchange, forgetting everything about the previous one.
    pub fn reset(&mut self, method: Method) {
        *self = Self::new(method);
    }

    /// Continues parsing the current message. `buf` always starts at the current message start and only grows
    /// between calls; scanning resumes where the last call stopped (O(head bytes) in total).
    ///
    /// # Errors
    ///
    /// `TooLarge(Headers)` when the head (interim heads included) or the status line exceeds its limit;
    /// `Protocol(SmugglingRisk)` for conflicting or unsupported framing; `Protocol(MalformedResponse)` for any other
    /// grammar or count violation; `Engine(Internal)` when called after `Complete` or an error without [`reset`], or
    /// with a `buf` shorter than the part already examined. After an error the parser stays failed until `reset`.
    ///
    /// [`reset`]: Self::reset
    pub fn parse(&mut self, buf: &[u8]) -> Result<ParseStatus, ErrorCode> {
        let result = self.step(buf);
        if result.is_err() {
            self.phase = Phase::Failed;
        }
        result
    }

    fn step(&mut self, buf: &[u8]) -> Result<ParseStatus, ErrorCode> {
        let budget = H1_LIMITS
            .max_head_bytes
            .checked_sub(self.interim_bytes)
            .ok_or(INTERNAL)?;
        // Bytes past the budget are never examined, so a split input sees exactly what a whole one sees.
        let window = buf.get(..buf.len().min(budget)).ok_or(INTERNAL)?;
        loop {
            let next = match self.phase {
                Phase::StatusPrefix => self.status_prefix(window)?,
                Phase::StatusReason => self.status_reason(window)?,
                Phase::LineStart => self.at_line_start(window)?,
                Phase::Name => self.name(window)?,
                Phase::Value => self.value(window)?,
                Phase::Done | Phase::Failed => return Err(INTERNAL),
            };
            match next {
                None => {}
                // The window ends before the head does: it cannot end within the budget once the buffer reaches it.
                Some(ParseStatus::Partial) if buf.len() >= budget => return Err(TOO_LARGE),
                Some(status) => return Ok(status),
            }
        }
    }

    /// `HTTP/1.0 DDD` or `HTTP/1.1 DDD`, checked at every byte that has arrived, so non-HTTP bytes (a TLS record,
    /// HTML, HTTP/0.9) fail at their first byte. Re-checks at most 12 bytes per call while incomplete.
    fn status_prefix(&mut self, window: &[u8]) -> Result<Option<ParseStatus>, ErrorCode> {
        let valid = |prefix: &[u8]| {
            prefix
                .iter()
                .enumerate()
                .all(|(i, &b)| status_prefix_byte(i, b))
        };
        // The complete prefix is checked as a fixed-size array (unrolled, one comparison per byte); a partial one byte
        // by byte as it arrives.
        let Some(&prefix) = window.first_chunk::<STATUS_PREFIX_LEN>() else {
            return if valid(window) {
                Ok(Some(ParseStatus::Partial))
            } else {
                Err(MALFORMED)
            };
        };
        if !valid(&prefix) {
            return Err(MALFORMED);
        }
        let [.., minor, _, d1, d2, d3] = prefix;
        let version = if minor == b'1' {
            HttpVersion::Http11
        } else {
            HttpVersion::Http10
        };
        let digit = |d: u8| u16::from(d.wrapping_sub(b'0'));
        // Each digit is 0..=9 (validated above), so the value is at most 999: the wrapping operations never wrap.
        let status = digit(d1)
            .wrapping_mul(100)
            .wrapping_add(digit(d2).wrapping_mul(10))
            .wrapping_add(digit(d3));
        let line = StatusLine { status, version };
        if !(100..=599).contains(&status)
            || status == 101
            || (line.is_interim() && self.interim_count >= H1_LIMITS.max_interim_responses)
        {
            return Err(MALFORMED);
        }
        self.line = line;
        self.pos = STATUS_PREFIX_LEN;
        self.phase = Phase::StatusReason;
        Ok(None)
    }

    /// CRLF (empty reason) or SP and a `FIELD_CONTENT` reason, ending in CRLF within the status-line limit. Bytes at or
    /// after the limit are never examined.
    fn status_reason(&mut self, window: &[u8]) -> Result<Option<ParseStatus>, ErrorCode> {
        // The status line starts at offset 0 of the message.
        let limit = H1_LIMITS.max_status_line_bytes;
        let line = window.get(..window.len().min(limit)).ok_or(INTERNAL)?;
        if self.pos == STATUS_PREFIX_LEN {
            // A buffer shorter than the examined prefix is the caller's contract violation, not the peer's.
            match line.get(STATUS_PREFIX_LEN..).ok_or(INTERNAL)? {
                [b' ', ..] => self.pos = REASON_START,
                [b'\r', ..] => {}
                [] => return Ok(Some(ParseStatus::Partial)),
                _ => return Err(MALFORMED),
            }
        }
        let rest = line.get(self.pos..).ok_or(INTERNAL)?;
        let end = checked_add(self.pos, value_len(rest))?;
        match line.get(end..) {
            Some([b'\r', b'\n', ..]) => {
                let start = checked_add(end, CRLF_LEN)?;
                self.headers_start = start;
                self.line_start = start;
                self.pos = start;
                self.phase = Phase::LineStart;
                Ok(None)
            }
            Some([] | [b'\r']) if window.len() >= limit => Err(TOO_LARGE),
            Some([] | [b'\r']) => {
                self.pos = end;
                Ok(Some(ParseStatus::Partial))
            }
            _ => Err(MALFORMED),
        }
    }

    /// The blank line that ends the head, or the first byte of a field name.
    fn at_line_start(&mut self, window: &[u8]) -> Result<Option<ParseStatus>, ErrorCode> {
        match window.get(self.pos..).ok_or(INTERNAL)? {
            [] | [b'\r'] => Ok(Some(ParseStatus::Partial)),
            [b'\r', b'\n', ..] => self.end_of_head().map(Some),
            [b'\r', ..] => Err(MALFORMED),
            _ => {
                self.fields = checked_add(self.fields, 1)?;
                if self.fields > H1_LIMITS.max_header_fields {
                    return Err(MALFORMED);
                }
                self.phase = Phase::Name;
                Ok(None)
            }
        }
    }

    /// A non-empty token directly followed by `:` (this rejects a leading SP/HTAB, whitespace before the colon, a
    /// missing colon, CR/LF and any non-token byte in the name).
    fn name(&mut self, window: &[u8]) -> Result<Option<ParseStatus>, ErrorCode> {
        let rest = window.get(self.pos..).ok_or(INTERNAL)?;
        let run = token_len(rest);
        let colon = checked_add(self.pos, run)?;
        match rest.get(run..) {
            Some([]) => {
                self.pos = colon;
                Ok(Some(ParseStatus::Partial))
            }
            Some([b':', ..]) if colon > self.line_start => {
                self.colon = colon;
                self.pos = checked_add(colon, 1)?;
                self.phase = Phase::Value;
                Ok(None)
            }
            _ => Err(MALFORMED),
        }
    }

    /// A `FIELD_CONTENT` run that must end in CRLF; the completed field line updates the framing facts.
    fn value(&mut self, window: &[u8]) -> Result<Option<ParseStatus>, ErrorCode> {
        let rest = window.get(self.pos..).ok_or(INTERNAL)?;
        let end = checked_add(self.pos, value_len(rest))?;
        match window.get(end..) {
            Some([] | [b'\r']) => {
                self.pos = end;
                Ok(Some(ParseStatus::Partial))
            }
            Some([b'\r', b'\n', ..]) => {
                let name = window.get(self.line_start..self.colon).ok_or(INTERNAL)?;
                let raw = window
                    .get(checked_add(self.colon, 1)?..end)
                    .ok_or(INTERNAL)?;
                self.facts.field(self.line, name, raw)?;
                let next = checked_add(end, CRLF_LEN)?;
                self.line_start = next;
                self.pos = next;
                self.phase = Phase::LineStart;
                Ok(None)
            }
            _ => Err(MALFORMED),
        }
    }

    /// `pos` is at the blank line's CR.
    fn end_of_head(&mut self) -> Result<ParseStatus, ErrorCode> {
        let head_len = checked_add(self.pos, CRLF_LEN)?;
        if self.line.is_interim() {
            *self = Self {
                facts: self.facts.after_interim(),
                interim_count: checked_add(self.interim_count, 1)?,
                interim_bytes: checked_add(self.interim_bytes, head_len)?,
                ..Self::fresh(self.head_request)
            };
            return Ok(ParseStatus::Interim { consumed: head_len });
        }
        let (framing, keep_alive) = self.facts.conclude(self.line, self.head_request);
        self.phase = Phase::Done;
        Ok(ParseStatus::Complete(ResponseHead {
            status: self.line.status,
            version: self.line.version,
            head_len,
            headers: self.headers_start..self.pos,
            framing,
            keep_alive,
        }))
    }
}
