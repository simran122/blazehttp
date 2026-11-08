//! Header templates (`docs/notes/request-writer.md` §6.2): `host: <authority>\r\n` plus the client's default fields,
//! pre-encoded and validated once on the control plane, bound to the authority they were built for.

use core::fmt;
use core::net::Ipv6Addr;

use super::fields::scan_fields;
use crate::generated::spec::{EngineDetail, ErrorCode, H1_LIMITS};
use crate::h1::{HEADERS_TOO_LARGE, INTERNAL};

const RESOURCE_EXHAUSTED: ErrorCode = ErrorCode::Engine(EngineDetail::ResourceExhausted);
const HOST_PREFIX: &[u8] = b"host: ";
const CRLF: &[u8] = b"\r\n";
const MAX_PORT: u32 = 65_535;
const MAX_PORT_DIGITS: usize = 5;

/// Pre-encoded `host: <authority>\r\n` + default fields, validated once on the control plane (note §6.2).
///
/// No `Clone`: the bytes may hold credentials, so copies are explicit. The shard owns it by value. Its bytes are
/// freed, not zeroed, on drop (note §6.2: off the wire and unreachable through safe code; accepted).
pub struct HeaderTemplate {
    /// `"host: " authority "\r\n" ("name: value\r\n")*`.
    bytes: Box<[u8]>,
    /// The authority is `bytes[HOST_PREFIX.len()..authority_end]`.
    authority_end: usize,
    /// The singleton names present (bits of the field scan).
    singletons: u8,
    /// Whether a field name is one of the client's `sensitiveHeaders`.
    sensitive: bool,
}

impl HeaderTemplate {
    /// Builds a template for `host` (WHATWG `url.host`: punycoded, lowercase, bracketed IPv6, no userinfo, default
    /// port omitted) and the default `fields` (`("name: value\r\n")*`, latin1 bytes) of which JS serialised
    /// `field_lines` fields. It is sensitive when a field name is one of `sensitive_names` (ASCII case folded, `_`
    /// read as `-`).
    ///
    /// # Errors
    ///
    /// - `TooLarge(Headers)` when the host line and `fields` exceed `maxRequestFieldsBytes`.
    /// - `Engine(Internal)` when `host` is not a valid authority, or `fields` has a malformed line, a forbidden name,
    ///   a repeated singleton name or a line count other than `field_lines`.
    /// - `Engine(ResourceExhausted)` when the bytes cannot be allocated.
    pub fn new(
        host: &[u8],
        fields: &[u8],
        field_lines: u16,
        sensitive_names: &[&[u8]],
    ) -> Result<Self, ErrorCode> {
        // Saturating: a sum past `usize::MAX` is past the limit too.
        let len = HOST_PREFIX
            .len()
            .saturating_add(host.len())
            .saturating_add(CRLF.len())
            .saturating_add(fields.len());
        if len > H1_LIMITS.max_request_fields_bytes {
            return Err(HEADERS_TOO_LARGE);
        }
        validate_host(host)?;
        let summary = scan_fields(fields, field_lines, 0, sensitive_names)?;
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
            sensitive: summary.sensitive,
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

    /// The pre-encoded `host` line and default fields.
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// The singleton names of the default fields, for the per-request check.
    pub(super) fn singletons(&self) -> u8 {
        self.singletons
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

/// `host = ( "[" IPv6address "]" / reg-name ) [ ":" port ]` as WHATWG serialises it (note §6.2): the IPv6 literal
/// must parse as an [`Ipv6Addr`] (no zone id, no IPv4 literal in brackets); a reg-name or IPv4 address is lowercase
/// `unreserved` / `sub-delims` without `%`, with no empty label (a trailing dot is allowed); the port is 1–65535
/// without a leading zero (WHATWG omits default ports and never pads).
fn validate_host(host: &[u8]) -> Result<(), ErrorCode> {
    let name_len = host_name_len(host).ok_or(INTERNAL)?;
    let (_, port) = host.split_at_checked(name_len).ok_or(INTERNAL)?;
    if port.is_empty() || is_port(port) {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

/// Length of the valid bracketed IPv6 literal or reg-name at the start of `host`; `None` when there is none.
fn host_name_len(host: &[u8]) -> Option<usize> {
    if let Some(rest) = host.strip_prefix(b"[") {
        let close = rest.iter().position(|&b| b == b']')?;
        let literal = core::str::from_utf8(rest.get(..close)?).ok()?;
        // `[` + literal + `]`; close < host.len().
        return literal
            .parse::<Ipv6Addr>()
            .is_ok()
            .then(|| close.wrapping_add(2));
    }
    let len = host.iter().take_while(|&&b| is_reg_name_byte(b)).count();
    let name = host.get(..len)?;
    // One trailing dot (a fully qualified name) is allowed; no other label may be empty.
    let labels = name
        .strip_suffix(b".")
        .filter(|rest| !rest.is_empty())
        .unwrap_or(name);
    let labels_ok = labels.split(|&b| b == b'.').all(|label| !label.is_empty());
    labels_ok.then_some(len)
}

/// RFC 3986 `unreserved` and `sub-delims` without `%`, lowercase only.
fn is_reg_name_byte(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._~!$&'()*+,;=".contains(&b)
}

/// `":" 1*5DIGIT`, no leading zero, value ≤ 65535.
fn is_port(port: &[u8]) -> bool {
    let Some(digits) = port.strip_prefix(b":") else {
        return false;
    };
    let value = digits.iter().try_fold(0_u32, |n, &b| {
        let digit = char::from(b).to_digit(10)?;
        n.checked_mul(10)?.checked_add(digit)
    });
    let leading_zero = digits.first() == Some(&b'0');
    !leading_zero
        && !digits.is_empty()
        && digits.len() <= MAX_PORT_DIGITS
        && value.is_some_and(|v| v <= MAX_PORT)
}
