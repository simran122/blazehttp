//! HTTP/1.1 request head serialisation (design §5.1 `RequestWriter`; decisions in `docs/notes/request-writer.md`).
//!
//! A [`HeaderTemplate`] holds `host` and the client's default fields, pre-encoded and validated once on the control
//! plane. Per request, [`write_request`] re-validates every byte that changes (the target and the per-request delta)
//! — the single choke point every outgoing head passes through (design §12, Injection) — and appends request line,
//! template, delta, the generated framing lines and the body into one reused buffer for one write. Steady state
//! allocates nothing: the buffer keeps its capacity and the one exact reservation per request is then a no-op.

use core::fmt;
use std::fmt::NumBuffer;

use super::grammar::{token_len, value_len};
use crate::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS, Method, TooLargeDetail};

/// A caller bypassed the JS layer's validation (P2 reports `INVALID_CONFIG` first), or broke the writer's contract.
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);
const RESOURCE_EXHAUSTED: ErrorCode = ErrorCode::Engine(EngineDetail::ResourceExhausted);
const URL_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Url);
const HEADERS_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);

const HOST_PREFIX: &[u8] = b"host: ";
const CRLF: &[u8] = b"\r\n";
const FIELD_SEPARATOR: &[u8] = b": ";
const SP: &[u8] = b" ";
const VERSION_SUFFIX: &[u8] = b" HTTP/1.1\r\n";
const CONTENT_LENGTH_PREFIX: &[u8] = b"content-length: ";
const CONNECTION_CLOSE: &[u8] = b"connection: close\r\n";
/// `OPTIONS`, the longest `Method::as_str()` (checked by a unit test).
const LONGEST_METHOD: usize = 7;
const MAX_PORT: u32 = 65_535;
const MAX_PORT_DIGITS: usize = 5;

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

/// Pre-encoded `host: <authority>\r\n` + default fields, validated once on the control plane (note §6.2).
///
/// No `Clone`: the bytes may hold credentials, so copies are explicit. The shard owns it by value.
pub struct HeaderTemplate {
    /// `"host: " authority "\r\n" ("name: value\r\n")*`.
    bytes: Box<[u8]>,
    /// The authority is `bytes[HOST_PREFIX.len()..authority_end]`.
    authority_end: usize,
    /// The singleton names present (bits of [`NameClass::Singleton`]).
    singletons: u8,
    /// Whether a field name is one of the client's `sensitiveHeaders`.
    sensitive: bool,
}

impl HeaderTemplate {
    /// Builds a template for `host` (WHATWG `url.host`: punycoded, lowercase, bracketed IPv6, no userinfo) and the
    /// default `fields` (`("name: value\r\n")*`, latin1 bytes). It is sensitive when a field name equals, ignoring
    /// ASCII case, one of `sensitive_names`.
    ///
    /// # Errors
    ///
    /// - `TooLarge(Headers)` when the host line and `fields` exceed `maxRequestFieldsBytes`.
    /// - `Engine(Internal)` when `host` is not a valid authority, or `fields` has a malformed line, a forbidden name
    ///   or a repeated singleton name.
    /// - `Engine(ResourceExhausted)` when the bytes cannot be allocated.
    pub fn new(host: &[u8], fields: &[u8], sensitive_names: &[&[u8]]) -> Result<Self, ErrorCode> {
        let len = HOST_PREFIX
            .len()
            .saturating_add(host.len())
            .saturating_add(CRLF.len())
            .saturating_add(fields.len());
        if len > H1_LIMITS.max_request_fields_bytes {
            return Err(HEADERS_TOO_LARGE);
        }
        validate_host(host)?;
        let summary = scan_fields(fields, 0)?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(len)
            .map_err(|_| RESOURCE_EXHAUSTED)?;
        for part in [HOST_PREFIX, host, CRLF, fields] {
            bytes.extend_from_slice(part);
        }
        Ok(Self {
            bytes: bytes.into_boxed_slice(),
            // ≤ the 64 KiB limit checked above: cannot overflow.
            authority_end: HOST_PREFIX.len().saturating_add(host.len()),
            singletons: summary.singletons,
            sensitive: names_any(fields, sensitive_names),
        })
    }

    /// The authority this template was built for; it serves only connections to that authority.
    #[must_use]
    pub fn authority(&self) -> &[u8] {
        self.bytes
            .get(HOST_PREFIX.len()..self.authority_end)
            .unwrap_or_default()
    }

    /// Whether a default field is one of the client's `sensitiveHeaders` (the registry releases such templates at
    /// once, design §11.2).
    #[must_use]
    pub fn is_sensitive(&self) -> bool {
        self.sensitive
    }
}

/// Lengths and flags only: the bytes may hold credentials and are never printed.
impl fmt::Debug for HeaderTemplate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HeaderTemplate")
            .field("len", &self.bytes.len())
            .field("authority_len", &self.authority().len())
            .field("sensitive", &self.sensitive)
            .finish_non_exhaustive()
    }
}

/// One request's per-request parts, borrowed from the submission arena.
///
/// A passive parameter object with no invariants and no state changes, so its fields are `pub` (the design §13.2
/// rule 1 exception); [`write_request`] validates every field.
pub struct RequestParts<'a> {
    pub method: Method,
    /// Origin-form target: `/` first, visible ASCII except `#`.
    pub target: &'a [u8],
    /// Per-request fields `("name: value\r\n")*`, latin1 bytes.
    pub headers: &'a [u8],
    /// How many fields the JS layer serialised into `headers`; a mismatch means a value smuggled a line.
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
/// # Errors
///
/// Nothing is written on error and `out` is left empty (capacity kept). The first failing check wins:
/// 1. `Engine(Internal)`: the body exceeds `maxCopiedBodyBytes` (the connection's `try_submit` was bypassed);
/// 2. `TooLarge(Url)`: the target exceeds `maxRequestTargetBytes`;
/// 3. `TooLarge(Headers)`: template plus `headers` exceed `maxRequestFieldsBytes`;
/// 4. `Engine(Internal)`: `conn_authority` is not the template's authority;
/// 5. `Engine(Internal)`: `HEAD` with a body;
/// 6. `Engine(Internal)`: the target is not origin-form visible ASCII without `#`;
/// 7. `Engine(Internal)`: a malformed header line, a forbidden name, a repeated singleton (template included), or
///    a line count other than `header_lines`;
/// 8. `Engine(ResourceExhausted)`: the buffer cannot grow to the request's length.
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
    let delta = scan_fields(parts.headers, template.singletons)?;
    if delta.lines != usize::from(parts.header_lines) {
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
    if template.bytes.len().saturating_add(parts.headers.len()) > H1_LIMITS.max_request_fields_bytes
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
        &*template.bytes,
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

/// Origin-form: `/` first, then only visible ASCII (0x21–0x7E) other than `#` (note §6.3). Branch-free over the bytes.
#[expect(
    clippy::needless_bitwise_bool,
    reason = "`&` keeps the fold branch-free so it vectorises (note §6.3); every operand is a cheap comparison"
)]
fn validate_target(target: &[u8]) -> Result<(), ErrorCode> {
    let origin_form = target.first() == Some(&b'/');
    let visible = target
        .iter()
        .fold(true, |ok, &b| ok & b.is_ascii_graphic() & (b != b'#'));
    if origin_form & visible {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

/// What a valid field block holds.
struct FieldSummary {
    /// `seen` plus the singleton names of the block.
    singletons: u8,
    lines: usize,
}

/// Validates a field block `("name: value\r\n")*` in one pass: line grammar, forbidden names, and singleton names at
/// most once counting `seen` (the template's) — O(block), no allocation.
fn scan_fields(block: &[u8], seen: u8) -> Result<FieldSummary, ErrorCode> {
    let mut summary = FieldSummary {
        singletons: seen,
        lines: 0,
    };
    for name in FieldNames(block) {
        summary.singletons = admit(classify(name?), summary.singletons)?;
        // At most one line per 5 bytes of a block whose length fits `usize`: cannot wrap.
        summary.lines = summary.lines.wrapping_add(1);
    }
    Ok(summary)
}

/// Whether a name of the (already valid) `block` equals one of `names`, ignoring ASCII case.
fn names_any(block: &[u8], names: &[&[u8]]) -> bool {
    FieldNames(block)
        .flatten()
        .any(|name| names.iter().any(|n| name.eq_ignore_ascii_case(n)))
}

/// The names of a field block's lines, each line checked against `token ": " *FIELD_CONTENT CRLF`; the first
/// malformed line yields an error and ends the iteration.
struct FieldNames<'a>(&'a [u8]);

impl<'a> Iterator for FieldNames<'a> {
    type Item = Result<&'a [u8], ErrorCode>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.0.is_empty() {
            return None;
        }
        let line = split_field_line(self.0);
        self.0 = line.map_or(&[], |(_, rest)| rest);
        Some(line.map(|(name, _)| name))
    }
}

/// Splits the first field line off `block`: its name and the bytes after its CRLF.
fn split_field_line(block: &[u8]) -> Result<(&[u8], &[u8]), ErrorCode> {
    let (name, after_name) = block.split_at_checked(token_len(block)).ok_or(INTERNAL)?;
    if name.is_empty() {
        return Err(INTERNAL);
    }
    let value = after_name.strip_prefix(FIELD_SEPARATOR).ok_or(INTERNAL)?;
    let (_, after_value) = value.split_at_checked(value_len(value)).ok_or(INTERNAL)?;
    let rest = after_value.strip_prefix(CRLF).ok_or(INTERNAL)?;
    Ok((name, rest))
}

/// How the writer treats a field name.
#[derive(Clone, Copy)]
enum NameClass {
    /// Framing, routing and hop-by-hop names that only the writer (or nobody) may produce.
    Forbidden,
    /// A name allowed at most once across template and delta; the `u8` is its bit.
    Singleton(u8),
    Other,
}

const AUTHORIZATION: u8 = 1;
const COOKIE: u8 = 1 << 1;
const CONTENT_TYPE: u8 = 1 << 2;
const USER_AGENT: u8 = 1 << 3;
const FORBIDDEN: NameClass = NameClass::Forbidden;

/// Classifies `name` in O(1): a switch on its length, then at most three candidates compared ignoring ASCII case,
/// with `_` in `name` standing for `-` in the candidate. A match through an underscore spelling is always
/// `Forbidden`: gateways that map `_` to `-` would turn it into the real field (note §6.4, X9).
fn classify(name: &[u8]) -> NameClass {
    let candidates: &[(&[u8], NameClass)] = match name.len() {
        2 => &[(b"te", FORBIDDEN)],
        4 => &[(b"host", FORBIDDEN)],
        6 => &[
            (b"expect", FORBIDDEN),
            (b"cookie", NameClass::Singleton(COOKIE)),
        ],
        7 => &[(b"upgrade", FORBIDDEN)],
        10 => &[
            (b"connection", FORBIDDEN),
            (b"keep-alive", FORBIDDEN),
            (b"user-agent", NameClass::Singleton(USER_AGENT)),
        ],
        12 => &[(b"content-type", NameClass::Singleton(CONTENT_TYPE))],
        13 => &[(b"authorization", NameClass::Singleton(AUTHORIZATION))],
        14 => &[(b"content-length", FORBIDDEN)],
        16 => &[(b"proxy-connection", FORBIDDEN)],
        17 => &[(b"transfer-encoding", FORBIDDEN)],
        19 => &[(b"proxy-authorization", FORBIDDEN)],
        _ => return NameClass::Other,
    };
    candidates
        .iter()
        .find_map(|&(canonical, class)| match spelling(name, canonical) {
            Spelling::Exact => Some(class),
            Spelling::Underscore => Some(FORBIDDEN),
            Spelling::Different => None,
        })
        .unwrap_or(NameClass::Other)
}

/// How a name of the same length spells a lowercase canonical name.
enum Spelling {
    Exact,
    /// Equal once `_` is read as `-`.
    Underscore,
    Different,
}

fn spelling(name: &[u8], canonical: &[u8]) -> Spelling {
    let mut underscore = false;
    for (&b, &c) in name.iter().zip(canonical) {
        if b.to_ascii_lowercase() == c {
            continue;
        }
        if b == b'_' && c == b'-' {
            underscore = true;
            continue;
        }
        return Spelling::Different;
    }
    if underscore {
        Spelling::Underscore
    } else {
        Spelling::Exact
    }
}

/// `seen` plus the name of `class`, or an error for a forbidden name or a repeated singleton.
fn admit(class: NameClass, seen: u8) -> Result<u8, ErrorCode> {
    match class {
        NameClass::Forbidden => Err(INTERNAL),
        NameClass::Singleton(bit) if seen & bit != 0 => Err(INTERNAL),
        NameClass::Singleton(bit) => Ok(seen | bit),
        NameClass::Other => Ok(seen),
    }
}

/// `host = ( "[" 1*( HEXDIG / ":" / "." ) "]" / 1*reg-name-byte ) [ ":" 1*5DIGIT ]`, port ≤ 65535, no `%`
/// (note §6.2: WHATWG has already decoded or rejected percent-encoding and stripped zone ids).
fn validate_host(host: &[u8]) -> Result<(), ErrorCode> {
    let name_len = host_name_len(host).ok_or(INTERNAL)?;
    let (_, port) = host.split_at_checked(name_len).ok_or(INTERNAL)?;
    if port.is_empty() || is_port(port) {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

/// Length of the bracketed IPv6 literal or reg-name/IPv4 at the start of `host`; `None` when there is none.
fn host_name_len(host: &[u8]) -> Option<usize> {
    if let Some(rest) = host.strip_prefix(b"[") {
        let close = rest.iter().position(|&b| b == b']')?;
        let literal = rest.get(..close)?;
        let valid = !literal.is_empty()
            && literal
                .iter()
                .all(|&b| b.is_ascii_hexdigit() || b == b':' || b == b'.');
        // `[` + literal + `]`; close < host.len().
        return valid.then(|| close.wrapping_add(2));
    }
    let len = host.iter().take_while(|&&b| is_reg_name_byte(b)).count();
    (len != 0).then_some(len)
}

/// RFC 3986 `unreserved` and `sub-delims` without `%`.
fn is_reg_name_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b"-._~!$&'()*+,;=".contains(&b)
}

/// `":" 1*5DIGIT` with a value ≤ 65535.
fn is_port(port: &[u8]) -> bool {
    let Some(digits) = port.strip_prefix(b":") else {
        return false;
    };
    let value = digits.iter().try_fold(0_u32, |n, &b| {
        let digit = char::from(b).to_digit(10)?;
        n.checked_mul(10)?.checked_add(digit)
    });
    !digits.is_empty() && digits.len() <= MAX_PORT_DIGITS && value.is_some_and(|v| v <= MAX_PORT)
}

#[cfg(test)]
mod tests {
    use super::{LONGEST_METHOD, NameClass, classify};
    use crate::generated::spec::Method;

    #[test]
    fn longest_method_covers_every_method() {
        let longest = (0..)
            .map_while(Method::from_wire)
            .map(|m| m.as_str().len())
            .max();
        assert_eq!(longest, Some(LONGEST_METHOD));
    }

    #[test]
    fn content_length_formats_any_usize() {
        let mut digits = std::fmt::NumBuffer::new();
        assert_eq!(usize::MAX.format_into(&mut digits), usize::MAX.to_string());
        assert_eq!(0_usize.format_into(&mut digits), "0");
    }

    #[test]
    fn classify_distinguishes_classes_by_length_and_spelling() {
        assert!(matches!(classify(b"Te"), NameClass::Forbidden));
        assert!(matches!(classify(b"tf"), NameClass::Other));
        assert!(matches!(classify(b"COOKIE"), NameClass::Singleton(_)));
        assert!(matches!(classify(b"user_agent"), NameClass::Forbidden));
        assert!(matches!(classify(b"content-typ_"), NameClass::Other));
        assert!(matches!(classify(b"x-request-id"), NameClass::Other));
    }
}
