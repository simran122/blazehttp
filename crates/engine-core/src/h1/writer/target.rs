//! The request-target rule (`docs/notes/request-writer.md` §6.3): origin-form, holding only bytes that the WHATWG URL
//! serialiser emits for an `http(s)` URL in the component they sit in. Anything else did not come from a parsed URL,
//! so the writer refuses it rather than guess what a server makes of it.

use crate::generated::spec::ErrorCode;
use crate::h1::INTERNAL;

/// The byte may not appear in the path (before the first `?`): WHATWG turns `\` into `/` and percent-encodes `^`,
/// `` ` ``, `{` and `}` there (path percent-encode set).
const NOT_IN_PATH: u8 = 1;
/// The byte may not appear in the query: WHATWG percent-encodes `'` in the query of special URLs.
const NOT_IN_QUERY: u8 = 1 << 1;
/// Never emitted: controls, SP, DEL, non-ASCII, `#` (fragment), `"`, `<`, `>` (always percent-encoded).
const NEVER: u8 = NOT_IN_PATH | NOT_IN_QUERY;

/// The flags of every byte; 0 means "allowed in both components".
static TARGET_FLAGS: [u8; 256] = target_flags_table();

#[expect(
    clippy::indexing_slicing,
    reason = "const fn: `get_mut` is not const on 1.99.1; `b` stays below 0x7F by the loop condition"
)]
const fn target_flags_table() -> [u8; 256] {
    let mut table = [NEVER; 256];
    let mut b: u8 = b'!';
    while b <= b'~' {
        table[b as usize] = match b {
            b'#' | b'"' | b'<' | b'>' => NEVER,
            b'\\' | b'^' | b'`' | b'{' | b'}' => NOT_IN_PATH,
            b'\'' => NOT_IN_QUERY,
            _ => 0,
        };
        b = b.wrapping_add(1); // b ≤ `~` (0x7E): cannot wrap
    }
    table
}

/// The union of the flags of `bytes`: one table load and one OR per byte, no branch and no carried component state.
fn flags_of(bytes: &[u8]) -> u8 {
    bytes.iter().fold(0, |flags, &b| {
        flags | TARGET_FLAGS.get(usize::from(b)).copied().unwrap_or(NEVER)
    })
}

/// Origin-form: `/` first, then bytes the component allows. The common target holds no flagged byte, so one pass
/// decides it; only a target with a flagged byte is split at its first `?` and each part checked against its own
/// component.
///
/// # Errors
///
/// `Engine(Internal)` when the target is not origin-form as defined above.
pub(super) fn validate_target(target: &[u8]) -> Result<(), ErrorCode> {
    let components_ok = flags_of(target) == 0 || {
        let mut components = target.splitn(2, |&b| b == b'?');
        let path = components.next().unwrap_or_default();
        let query = components.next().unwrap_or_default();
        flags_of(path) & NOT_IN_PATH == 0 && flags_of(query) & NOT_IN_QUERY == 0
    };
    if target.first() == Some(&b'/') && components_ok {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

#[cfg(test)]
mod tests {
    use super::{NEVER, NOT_IN_PATH, NOT_IN_QUERY, TARGET_FLAGS};

    /// The table against the rule written out byte by byte.
    #[test]
    fn flags_follow_the_whatwg_percent_encode_sets() {
        for (b, &flags) in (0..=u8::MAX).zip(TARGET_FLAGS.iter()) {
            let expected = match b {
                b'#' | b'"' | b'<' | b'>' => NEVER,
                b'\\' | b'^' | b'`' | b'{' | b'}' => NOT_IN_PATH,
                b'\'' => NOT_IN_QUERY,
                _ if b.is_ascii_graphic() => 0,
                _ => NEVER,
            };
            assert_eq!(flags, expected, "{b:#04x}");
        }
    }
}
