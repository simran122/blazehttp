//! The template's `host` rule (`docs/notes/request-writer.md` §6.2): the authority exactly as the WHATWG URL
//! serialiser writes it for an `http(s)` URL, so one origin has one spelling and the byte-equality binding of the
//! template to its connection is sound.

use core::fmt;
use core::net::Ipv6Addr;
use core::ops::Range;

use crate::generated::spec::ErrorCode;
use crate::h1::INTERNAL;

const MAX_PORT: u32 = 65_535;
const MAX_PORT_DIGITS: usize = 5;
const IPV6_PIECES: usize = 8;

/// `host = ( "[" IPv6address "]" / reg-name ) [ ":" port ]` as WHATWG serialises it: the IPv6 literal in the URL
/// Standard's IPv6 serialisation (no zone id, no dotted IPv4 tail); a reg-name or IPv4 address is lowercase
/// `unreserved` / `sub-delims` without `%`, with no empty label (a trailing dot is allowed); the port is 1–65535
/// without a leading zero (WHATWG omits default ports and never pads).
///
/// # Errors
///
/// `Engine(Internal)` when `host` breaks the rule.
pub(super) fn validate_host(host: &[u8]) -> Result<(), ErrorCode> {
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
        let literal = rest.get(..close)?;
        let address = core::str::from_utf8(literal)
            .ok()?
            .parse::<Ipv6Addr>()
            .ok()?;
        // `[` + literal + `]`; close < host.len().
        return is_whatwg_ipv6(literal, address).then(|| close.wrapping_add(2));
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

/// Whether `literal` is `address` in the URL Standard's IPv6 serialisation (§3.6): lowercase hex pieces without
/// leading zeros, the first longest run of two or more zero pieces written as `::`. `Ipv6Addr`'s `Display` is not
/// that form for IPv4-mapped addresses, so the serialisation is written here and compared without allocating.
fn is_whatwg_ipv6(literal: &[u8], address: Ipv6Addr) -> bool {
    let mut expected = PrefixMatcher(Some(literal));
    write_whatwg_ipv6(&mut expected, address.segments()).is_ok() && expected.0 == Some(&[])
}

/// The URL Standard's IPv6 serializer.
fn write_whatwg_ipv6(out: &mut impl fmt::Write, pieces: [u16; IPV6_PIECES]) -> fmt::Result {
    let compress = longest_zero_run(pieces);
    for (index, piece) in pieces.into_iter().enumerate() {
        match &compress {
            Some(run) if run.start == index => {
                out.write_str(if index == 0 { "::" } else { ":" })?;
            }
            Some(run) if run.contains(&index) => {}
            _ => {
                write!(out, "{piece:x}")?;
                if index.wrapping_add(1) != IPV6_PIECES {
                    out.write_str(":")?;
                }
            }
        }
    }
    Ok(())
}

/// The first longest run of zero pieces, if it is at least two long.
fn longest_zero_run(pieces: [u16; IPV6_PIECES]) -> Option<Range<usize>> {
    let mut longest: Option<Range<usize>> = None;
    let mut start = None;
    // A non-zero sentinel after the last piece ends a trailing run.
    for (index, piece) in pieces.into_iter().chain([1]).enumerate() {
        match (piece == 0, start) {
            (true, None) => start = Some(index),
            (false, Some(first)) => {
                let run = first..index;
                if run.len() >= 2 && longest.as_ref().is_none_or(|best| run.len() > best.len()) {
                    longest = Some(run);
                }
                start = None;
            }
            _ => {}
        }
    }
    longest
}

/// A `fmt::Write` sink that consumes `self.0` while the written text matches it; `None` after a mismatch.
struct PrefixMatcher<'a>(Option<&'a [u8]>);

impl fmt::Write for PrefixMatcher<'_> {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.0 = self.0.and_then(|rest| rest.strip_prefix(s.as_bytes()));
        Ok(())
    }
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
