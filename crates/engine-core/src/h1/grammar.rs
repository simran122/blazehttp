//! HTTP/1.1 head byte classes and span scans (`docs/notes/response-parser.md` §6.2). The tables are the only source of
//! truth; `value_len`'s SWAR fast path only skips words the table would accept. `engine-simd` (P1a) replaces the two
//! span functions behind the same signatures.

#![deny(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]

/// RFC 9110 §5.6.2 `tchar`: DIGIT, ALPHA and `! # $ % & ' * + - . ^ _ | ~` plus the grave accent.
pub(crate) static TCHAR: [bool; 256] = build(&[
    (b'!', b'!'),
    (b'#', b'\''), // # $ % & '
    (b'*', b'+'),
    (b'-', b'.'),
    (b'0', b'9'),
    (b'A', b'Z'),
    (b'^', b'z'), // ^ _ ` and lower ALPHA
    (b'|', b'|'),
    (b'~', b'~'),
]);

/// RFC 9110 §5.5 field content (`field-vchar`, SP, HTAB; obs-text included), also RFC 9112 §4 `reason-phrase`.
/// Excludes CR, LF, NUL, the other control characters and DEL.
pub(crate) static FIELD_CONTENT: [bool; 256] = build(&[(b'\t', b'\t'), (b' ', b'~'), (0x80, 0xFF)]);

/// A 256-entry class table from inclusive byte ranges.
#[expect(
    clippy::indexing_slicing,
    reason = "const fn: `get`/`get_mut` are not const on 1.99.1; indices are bounded by the loop conditions"
)]
const fn build(ranges: &[(u8, u8)]) -> [bool; 256] {
    let mut table = [false; 256];
    let mut r = 0;
    while r < ranges.len() {
        let (first, last) = ranges[r];
        let mut b = first;
        loop {
            table[b as usize] = true;
            if b == last {
                break;
            }
            b = b.wrapping_add(1); // b < last ≤ 0xFF: cannot wrap
        }
        r = r.wrapping_add(1); // r < ranges.len(): cannot wrap
    }
    table
}

/// Whether `b` is in `table`; `get` on a `u8` index into 256 entries never misses (no bounds check survives).
#[inline]
pub(crate) fn class(table: &[bool; 256], b: u8) -> bool {
    table.get(usize::from(b)).copied().unwrap_or(false)
}

/// Length of the leading `TCHAR` run of `s`.
#[inline]
pub(crate) fn token_len(s: &[u8]) -> usize {
    s.iter().position(|&b| !class(&TCHAR, b)).unwrap_or(s.len())
}

const LANES: u64 = 0x0101_0101_0101_0101;
const HIGH_BITS: u64 = 0x8080_8080_8080_8080;

/// SWAR flags of a little-endian word: the high bit of each byte that may be below 0x20 or equal to 0x7F ("has less"
/// / "has zero" tests). Zero exactly when the word has no such byte, so a clean word is skipped. Borrows only
/// propagate upward from a truly flagged byte, so the **lowest** flag is exact; flags above it may be false (borrow
/// propagation), and an exact flag may still be HTAB (below 0x20 but field content). Obs-text (≥ 0x80) never sets a
/// flag: `!x` clears its high bit.
#[inline]
fn non_field_content_flags(x: u64) -> u64 {
    let below_space = x.wrapping_sub(LANES.wrapping_mul(0x20)) & !x & HIGH_BITS;
    let del = x ^ LANES.wrapping_mul(0x7F);
    let is_del = del.wrapping_sub(LANES) & !del & HIGH_BITS;
    below_space | is_del
}

/// Length of the leading `FIELD_CONTENT` run of `s`, eight bytes per step. A flagged word jumps straight to its lowest
/// (exact) flag with `trailing_zeros`; only when that byte is HTAB does the rest of the word go through the table.
#[inline]
pub(crate) fn value_len(s: &[u8]) -> usize {
    let (words, tail) = s.as_chunks::<8>();
    // `scanned` ≤ s.len() ≤ isize::MAX and every offset below is < 8, so the wrapping additions never wrap.
    let mut scanned: usize = 0;
    for word in words {
        let flags = non_field_content_flags(u64::from_le_bytes(*word));
        if flags != 0 {
            // `trailing_zeros` < 64, so the lane index is < 8 and fits any `usize`.
            let lane = (flags.trailing_zeros() / 8) as usize;
            let stop = if word.get(lane) == Some(&b'\t') {
                word.iter()
                    .skip(lane.wrapping_add(1))
                    .position(|&b| !class(&FIELD_CONTENT, b))
                    .map(|p| lane.wrapping_add(1).wrapping_add(p))
            } else {
                Some(lane)
            };
            if let Some(stop) = stop {
                return scanned.wrapping_add(stop);
            }
        }
        scanned = scanned.wrapping_add(8);
    }
    scanned.wrapping_add(
        tail.iter()
            .position(|&b| !class(&FIELD_CONTENT, b))
            .unwrap_or(tail.len()),
    )
}

#[cfg(test)]
mod tests {
    use super::{FIELD_CONTENT, TCHAR, class, token_len, value_len};

    fn reference_len(table: &[bool; 256], s: &[u8]) -> usize {
        s.iter().position(|&b| !class(table, b)).unwrap_or(s.len())
    }

    #[test]
    fn tables_match_the_rfc_classes() {
        for b in u8::MIN..=u8::MAX {
            let tchar = b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b);
            assert_eq!(class(&TCHAR, b), tchar, "tchar {b:#04x}");
            let field = b == b'\t' || (b' '..=b'~').contains(&b) || b >= 0x80;
            assert_eq!(class(&FIELD_CONTENT, b), field, "field content {b:#04x}");
        }
    }

    #[test]
    fn value_len_matches_the_table_for_every_byte_at_every_lane_and_in_the_tail() {
        // Two words and a three-byte tail.
        for b in u8::MIN..=u8::MAX {
            for at in 0..19 {
                let mut s = [b'a'; 19];
                if let Some(slot) = s.get_mut(at) {
                    *slot = b;
                }
                assert_eq!(
                    value_len(&s),
                    reference_len(&FIELD_CONTENT, &s),
                    "{b:#04x} at {at}"
                );
            }
        }
    }

    #[test]
    fn value_len_matches_the_table_for_every_adjacent_pair() {
        // Lanes 0/1, 6/7 and 7/tail of a word followed by a one-byte tail.
        for (first, second) in [(0, 1), (6, 7), (7, 8)] {
            for x in u8::MIN..=u8::MAX {
                for y in u8::MIN..=u8::MAX {
                    let mut s = [b'a'; 9];
                    if let Some(slot) = s.get_mut(first) {
                        *slot = x;
                    }
                    if let Some(slot) = s.get_mut(second) {
                        *slot = y;
                    }
                    assert_eq!(
                        value_len(&s),
                        reference_len(&FIELD_CONTENT, &s),
                        "{x:#04x} {y:#04x} at {first}/{second}"
                    );
                }
            }
        }
    }

    #[test]
    fn token_len_stops_at_the_first_non_tchar() {
        assert_eq!(token_len(b"Content-Length: 5"), 14);
        assert_eq!(token_len(b"abc"), 3);
        assert_eq!(token_len(b""), 0);
        assert_eq!(token_len(b" x"), 0);
    }

    #[test]
    fn value_len_of_short_and_empty_inputs() {
        assert_eq!(value_len(b""), 0);
        assert_eq!(value_len(b"ok\r\n"), 2);
        assert_eq!(value_len(b"0123456789abcdef\r"), 16);
        assert_eq!(value_len(b"tab\there and \x80 obs-text\r\n"), 23);
    }
}
