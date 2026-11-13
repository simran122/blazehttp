//! Header templates (`docs/notes/request-writer.md` §6.2): `host: <authority>\r\n` plus the client's default fields,
//! pre-encoded and validated once on the control plane, bound to the authority they were built for.

use core::fmt;

use super::fields::scan_fields;
use super::host::validate_host;
use super::{CRLF, RESOURCE_EXHAUSTED};
use crate::generated::spec::{ErrorCode, H1_LIMITS};
use crate::h1::HEADERS_TOO_LARGE;

const HOST_PREFIX: &[u8] = b"host: ";

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
