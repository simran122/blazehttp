//! HTTP/1.1 request head serialisation (design §5.1 `RequestWriter`; decisions in `docs/notes/request-writer.md`).
//!
//! A [`HeaderTemplate`] (`template.rs`) holds `host` and the client's default fields, pre-encoded and validated once
//! on the control plane. Per request, [`write_request`] re-validates every byte that changes (the target and the
//! per-request delta, `fields.rs`) — the single choke point every outgoing head passes through (design §12,
//! Injection) — and appends request line, template, delta, the generated framing lines and the body into one reused
//! buffer for one write. Steady state allocates nothing: the buffer keeps its capacity and the one exact reservation
//! per request is then a no-op.

mod fields;
mod host;
mod target;
mod template;

use core::fmt;
use std::fmt::NumBuffer;

use fields::{CONTENT_TYPE, scan_fields};
use target::validate_target;
pub use template::HeaderTemplate;

use crate::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS, Method, TooLargeDetail};
use crate::h1::{HEADERS_TOO_LARGE, INTERNAL};

const RESOURCE_EXHAUSTED: ErrorCode = ErrorCode::Engine(EngineDetail::ResourceExhausted);
const URL_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Url);

/// Shared with `fields.rs` and `template.rs`.
const CRLF: &[u8] = b"\r\n";
const SP: &[u8] = b" ";
const VERSION_SUFFIX: &[u8] = b" HTTP/1.1\r\n";
const CONTENT_LENGTH_PREFIX: &[u8] = b"content-length: ";
const CONNECTION_CLOSE: &[u8] = b"connection: close\r\n";
/// `OPTIONS`, the longest `Method::as_str()` (checked by a unit test).
const LONGEST_METHOD: usize = 7;

const _: () = assert!(
    H1_LIMITS.max_copied_body_bytes > 0,
    "MAX_P0_REQUEST_BYTES counts the digits of maxCopiedBodyBytes with ilog10, which needs a positive limit"
);

/// The largest request P0 can write: the three spec limits plus the fixed head overhead (longest method, SP,
/// ` HTTP/1.1` CRLF, the `content-length` line for the largest copied body, `connection: close`, the blank line).
/// The connection shrinks its write buffer to this size after a write, never below it (note §6.7, X5).
pub const MAX_P0_REQUEST_BYTES: usize = LONGEST_METHOD
    + SP.len()
    + H1_LIMITS.max_request_target_bytes
    + VERSION_SUFFIX.len()
    + H1_LIMITS.max_request_fields_bytes
    + CONTENT_LENGTH_PREFIX.len()
    + (H1_LIMITS.max_copied_body_bytes.ilog10() + 1) as usize
    + CRLF.len()
    + CONNECTION_CLOSE.len()
    + CRLF.len()
    + H1_LIMITS.max_copied_body_bytes;

/// One request's per-request parts, borrowed from the submission arena.
///
/// A passive parameter object with no invariants and no state changes, so its fields are `pub` (design §13.2 rule
/// 1); [`write_request`] validates every field.
pub struct RequestParts<'a> {
    pub method: Method,
    /// Origin-form target: `/` first, then only bytes WHATWG emits in that component (note §6.3, `target.rs`).
    pub target: &'a [u8],
    /// Per-request fields `("name: value\r\n")*`, latin1 bytes.
    pub headers: &'a [u8],
    /// How many fields the JS layer serialised into `headers` — the number of entries it iterated, never a count
    /// of the serialised bytes; a mismatch means a value smuggled a line.
    pub header_lines: u16,
    pub body: &'a [u8],
}

/// Lengths and counts only: target, headers and body may hold credentials and are never printed.
impl fmt::Debug for RequestParts<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RequestParts")
            .field("method", &self.method)
            .field("target_len", &self.target.len())
            .field("headers_len", &self.headers.len())
            .field("header_lines", &self.header_lines)
            .field("body_len", &self.body.len())
            .finish()
    }
}

/// Whether the connection may be reused after this exchange; the connection must honour it (note §10.1).
#[must_use]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reuse {
    Allowed,
    /// The head announced `connection: close`: read the response, then close; never pool.
    CloseAfterExchange,
}

/// Clears `out` and writes the whole request into it: request line, `template`, the validated `parts.headers`, the
/// generated `content-length` and `connection: close` lines when due, the blank line and the body. `out` keeps its
/// capacity and is reserved exactly once, so steady state does not allocate; it is never shrunk (the connection
/// does that after the write, note §6.7).
///
/// The writer enforces the singleton subset of repeated names only; one line per non-list field name, and no
/// repeat of a default (template) name in `headers`, is the JS layer's contract (note §10.4).
///
/// # Errors
///
/// Nothing is written on error and `out` is left empty (capacity kept). The first failing check wins:
/// 1. `Engine(Internal)`: the body exceeds `maxCopiedBodyBytes` (the connection's `try_submit` was bypassed);
/// 2. `TooLarge(Url)`: the target exceeds `maxRequestTargetBytes`;
/// 3. `TooLarge(Headers)`: template plus `headers` exceed `maxRequestFieldsBytes`;
/// 4. `Engine(Internal)`: `conn_authority` is not the template's authority;
/// 5. `Engine(Internal)`: `HEAD` with a body;
/// 6. `Engine(Internal)`: the target is not origin-form (note §6.3);
/// 7. `Engine(Internal)`: a malformed header line, a forbidden name, a repeated singleton (template included), a
///    line count other than `header_lines`, or `OPTIONS` content without a `content-type`;
/// 8. `Engine(ResourceExhausted)`: the buffer cannot grow to the request's length.
// Measured: inlining into the caller's loop takes ≈ 35% (`get`), ≈ 9% (`post1k`) and ≈ 5–12% (`varied`) less time,
// by writer/floor ratio (docs/perf/journal.md, 2026-11-08).
#[inline]
pub fn write_request(
    out: &mut Vec<u8>,
    parts: &RequestParts<'_>,
    template: &HeaderTemplate,
    conn_authority: &[u8],
) -> Result<Reuse, ErrorCode> {
    out.clear();
    let framing = check_request(parts, template, conn_authority)?;
    let mut digits = NumBuffer::new();
    let pieces = request_pieces(parts, template, framing, &mut digits);
    let total = pieces
        .iter()
        .try_fold(0_usize, |sum, piece| sum.checked_add(piece.len()))
        .ok_or(RESOURCE_EXHAUSTED)?;
    out.try_reserve_exact(total)
        .map_err(|_| RESOURCE_EXHAUSTED)?;
    for piece in pieces {
        out.extend_from_slice(piece);
    }
    Ok(framing.reuse())
}

/// Every check of `write_request` in precedence order (note §7); returns the request's framing.
fn check_request(
    parts: &RequestParts<'_>,
    template: &HeaderTemplate,
    conn_authority: &[u8],
) -> Result<RequestFraming, ErrorCode> {
    check_limits(parts, template)?;
    if conn_authority != template.authority() {
        return Err(INTERNAL);
    }
    let framing = framing(parts.method, parts.body.len())?;
    validate_target(parts.target)?;
    let delta = scan_fields(
        parts.headers,
        parts.header_lines,
        template.singletons(),
        &[],
    )?;
    // RFC 9110 §9.3.7: a client that sends content in OPTIONS MUST send a valid Content-Type.
    if parts.method == Method::Options
        && !parts.body.is_empty()
        && delta.singletons & CONTENT_TYPE == 0
    {
        return Err(INTERNAL);
    }
    Ok(framing)
}

/// The O(1) size checks, which also bound every later scan.
fn check_limits(parts: &RequestParts<'_>, template: &HeaderTemplate) -> Result<(), ErrorCode> {
    if parts.body.len() > H1_LIMITS.max_copied_body_bytes {
        return Err(INTERNAL);
    }
    if parts.target.len() > H1_LIMITS.max_request_target_bytes {
        return Err(URL_TOO_LARGE);
    }
    if template.bytes().len().saturating_add(parts.headers.len())
        > H1_LIMITS.max_request_fields_bytes
    {
        return Err(HEADERS_TOO_LARGE);
    }
    Ok(())
}

/// The request's bytes in wire order (note §6.5 Order); `total` and the appends both come from this one list.
fn request_pieces<'a>(
    parts: &RequestParts<'a>,
    template: &'a HeaderTemplate,
    framing: RequestFraming,
    digits: &'a mut NumBuffer<usize>,
) -> [&'a [u8]; 12] {
    let (length_prefix, length, length_end): (&[u8], &[u8], &[u8]) = if framing.content_length {
        let length = parts.body.len().format_into(digits).as_bytes();
        (CONTENT_LENGTH_PREFIX, length, CRLF)
    } else {
        (&[], &[], &[])
    };
    let close: &[u8] = if framing.close { CONNECTION_CLOSE } else { &[] };
    [
        parts.method.as_str().as_bytes(),
        SP,
        parts.target,
        VERSION_SUFFIX,
        template.bytes(),
        parts.headers,
        length_prefix,
        length,
        length_end,
        close,
        CRLF,
        parts.body,
    ]
}

/// The generated framing lines of a request.
#[derive(Clone, Copy)]
struct RequestFraming {
    content_length: bool,
    close: bool,
}

impl RequestFraming {
    fn reuse(self) -> Reuse {
        if self.close {
            Reuse::CloseAfterExchange
        } else {
            Reuse::Allowed
        }
    }
}

/// The single place that decides framing (note §6.5): `content-length` for methods that define content (even
/// when 0) and for any body; a body on `GET`/`DELETE`/`OPTIONS` also closes the connection; `HEAD` with a body is
/// an error.
fn framing(method: Method, body_len: usize) -> Result<RequestFraming, ErrorCode> {
    let has_body = body_len != 0;
    match method {
        Method::Post | Method::Put | Method::Patch | Method::Query => Ok(RequestFraming {
            content_length: true,
            close: false,
        }),
        Method::Get | Method::Delete | Method::Options => Ok(RequestFraming {
            content_length: has_body,
            close: has_body,
        }),
        Method::Head if has_body => Err(INTERNAL),
        Method::Head => Ok(RequestFraming {
            content_length: false,
            close: false,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::LONGEST_METHOD;
    use crate::generated::spec::Method;

    #[test]
    fn longest_method_covers_every_method() {
        let longest = (0..)
            .map_while(Method::from_wire)
            .map(|m| m.as_str().len())
            .max();
        assert_eq!(longest, Some(LONGEST_METHOD));
    }
}
