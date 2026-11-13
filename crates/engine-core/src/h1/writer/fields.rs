//! Field blocks `("name: value\r\n")*` and the writer's name policy (`docs/notes/request-writer.md` §6.4): one pass
//! checks every line's grammar, refuses forbidden names, admits each singleton name at most once, flags sensitive
//! names and compares the line count with the count the JS layer serialised. Used for the template's default fields
//! (once) and for every request's delta.

use super::CRLF;
use crate::generated::spec::ErrorCode;
use crate::h1::INTERNAL;
use crate::h1::grammar::{token_len, value_len};

const FIELD_SEPARATOR: &[u8] = b": ";
/// Every `proxy-` field is forbidden (Fetch's forbidden prefix): proxy credentials and hop-by-hop proxy fields only
/// ever come from the proxy configuration (P1b).
const PROXY_PREFIX: &[u8] = b"proxy-";

/// Bits of the singleton names, at most once across template and delta.
const AUTHORIZATION: u8 = 1;
const COOKIE: u8 = 1 << 1;
pub(super) const CONTENT_TYPE: u8 = 1 << 2;
const USER_AGENT: u8 = 1 << 3;

/// What a valid field block holds.
pub(super) struct FieldSummary {
    /// The block's singleton names, plus those already seen.
    pub(super) singletons: u8,
    /// Whether a name is one of the given sensitive names (same spelling rule as the name policy).
    pub(super) sensitive: bool,
}

/// Validates `block` in one pass, O(block) with no allocation: line grammar, forbidden names, singleton names at
/// most once counting `seen`, and exactly `expected_lines` lines — the count JS serialised, so a CRLF smuggled inside
/// a value that forms a well-formed extra line is a mismatch (X3). Sensitive names are flagged on the way.
///
/// # Errors
///
/// `Engine(Internal)` for any violation.
pub(super) fn scan_fields(
    block: &[u8],
    expected_lines: u16,
    seen: u8,
    sensitive_names: &[&[u8]],
) -> Result<FieldSummary, ErrorCode> {
    let mut summary = FieldSummary {
        singletons: seen,
        sensitive: false,
    };
    let mut lines: usize = 0;
    for name in FieldNames(block) {
        let name = name?;
        summary.singletons = admit(classify(name), summary.singletons)?;
        summary.sensitive |= sensitive_names.iter().any(|s| same_name(name, s));
        // At most one line per 5 bytes of a block whose length fits `usize`: cannot wrap.
        lines = lines.wrapping_add(1);
    }
    if lines == usize::from(expected_lines) {
        Ok(summary)
    } else {
        Err(INTERNAL)
    }
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NameClass {
    /// Framing, routing and hop-by-hop names that only the writer (or nobody) may produce.
    Forbidden,
    /// A name allowed at most once across template and delta; the `u8` is its bit.
    Singleton(u8),
    Other,
}

const FORBIDDEN: NameClass = NameClass::Forbidden;

/// Classifies `name` in O(1): the `proxy-` prefix, then a switch on the length and at most three candidates. Names
/// compare by [`same_name`]; a match through an underscore spelling is always `Forbidden`, because gateways that map
/// `_` to `-` would turn it into the real field (note §6.4, X9).
fn classify(name: &[u8]) -> NameClass {
    if name
        .get(..PROXY_PREFIX.len())
        .is_some_and(|prefix| same_name(prefix, PROXY_PREFIX))
    {
        return FORBIDDEN;
    }
    let candidates: &[(&[u8], NameClass)] = match name.len() {
        2 => &[(b"te", FORBIDDEN)],
        4 => &[(b"host", FORBIDDEN)],
        6 => &[
            (b"expect", FORBIDDEN),
            (b"cookie", NameClass::Singleton(COOKIE)),
        ],
        7 => &[(b"upgrade", FORBIDDEN), (b"trailer", FORBIDDEN)],
        10 => &[
            (b"connection", FORBIDDEN),
            (b"keep-alive", FORBIDDEN),
            (b"user-agent", NameClass::Singleton(USER_AGENT)),
        ],
        12 => &[(b"content-type", NameClass::Singleton(CONTENT_TYPE))],
        13 => &[(b"authorization", NameClass::Singleton(AUTHORIZATION))],
        14 => &[
            (b"content-length", FORBIDDEN),
            (b"http2-settings", FORBIDDEN),
        ],
        17 => &[(b"transfer-encoding", FORBIDDEN)],
        _ => return NameClass::Other,
    };
    candidates
        .iter()
        .find(|(canonical, _)| same_name(name, canonical))
        .map_or(NameClass::Other, |&(_, class)| {
            if name.contains(&b'_') {
                FORBIDDEN
            } else {
                class
            }
        })
}

/// Whether two field names are the same field once ASCII case is folded and `_` is read as `-` (the spelling rule of
/// the name policy and of the sensitive-name match).
///
/// The plain case-insensitive compare answers the common case first; the folding loop runs only when it fails
/// (measured: the fold alone cost ≈ 10–15 ns per `post1k` request, docs/perf/journal.md).
fn same_name(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
        || (a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| fold(x) == fold(y)))
}

fn fold(b: u8) -> u8 {
    if b == b'_' {
        b'-'
    } else {
        b.to_ascii_lowercase()
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

#[cfg(test)]
mod tests {
    use super::{NameClass, classify, same_name};

    /// (name, class): exact, case and underscore spellings, prefix, near misses.
    const CLASSES: [(&[u8], NameClass); 12] = [
        (b"Te", NameClass::Forbidden),
        (b"tf", NameClass::Other),
        (b"COOKIE", NameClass::Singleton(super::COOKIE)),
        (b"user_agent", NameClass::Forbidden),
        (b"content-typ_", NameClass::Other),
        (b"x-request-id", NameClass::Other),
        (b"Trailer", NameClass::Forbidden),
        (b"HTTP2_Settings", NameClass::Forbidden),
        (b"Proxy-Anything", NameClass::Forbidden),
        (b"proxy_", NameClass::Forbidden),
        (b"proxy", NameClass::Other),
        (b"proxyx-a", NameClass::Other),
    ];

    #[test]
    fn classify_distinguishes_classes_by_prefix_length_and_spelling() {
        for (name, class) in CLASSES {
            assert_eq!(classify(name), class, "{}", name.escape_ascii());
        }
    }

    #[test]
    fn same_name_folds_case_and_underscore_symmetrically() {
        assert!(same_name(b"X_Api-KEY", b"x-api_key"));
        assert!(!same_name(b"x-api-key", b"x-api-ke"));
        assert!(!same_name(b"x-api-key", b"x.api-key"));
    }
}
