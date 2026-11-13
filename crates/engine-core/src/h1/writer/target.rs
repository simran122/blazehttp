//! The request-target rule (`docs/notes/request-writer.md` §6.3): origin-form, holding only bytes that the WHATWG URL
//! serialiser emits for an `http(s)` URL in the component they sit in. Anything else did not come from a parsed URL,
//! so the writer refuses it rather than guess what a server makes of it.

use crate::generated::spec::ErrorCode;
use crate::h1::INTERNAL;

/// Origin-form: `/` first, then bytes the component allows. The common target holds only bytes allowed in every
/// component, which one branch-free fold decides; only a target holding a component-specific byte is split at its
/// first `?` and each part checked against its own component (rare: such a byte is percent-encoded by WHATWG in at
/// least one component).
///
/// # Errors
///
/// `Engine(Internal)` when the target is not origin-form as defined above.
pub(super) fn validate_target(target: &[u8]) -> Result<(), ErrorCode> {
    let origin_form = target.first() == Some(&b'/');
    let common = target
        .iter()
        .fold(true, |ok, &b| ok & allowed_everywhere(b));
    if origin_form && (common || components_allowed(target)) {
        Ok(())
    } else {
        Err(INTERNAL)
    }
}

/// Whether WHATWG emits `b` in the path and in the query: visible ASCII except `#`, `"`, `<`, `>` (never emitted)
/// and the component-specific bytes of [`allowed_in_path`] and [`allowed_in_query`]. Comparisons joined with `&`
/// only: a table lookup per byte keeps the fold scalar, and a `match` compiles to byte-dependent branches (both
/// measured slower, docs/perf/journal.md, 2026-11-13).
#[expect(
    clippy::needless_bitwise_bool,
    reason = "`&` keeps the per-byte test branch-free so the fold vectorises; every operand is a cheap comparison"
)]
fn allowed_everywhere(b: u8) -> bool {
    emitted(b) & (b != b'\\') & (b != b'^') & (b != b'`') & (b != b'{') & (b != b'}') & (b != b'\'')
}

/// Visible ASCII except `#` (a fragment), `"`, `<` and `>`, which WHATWG percent-encodes in every component.
#[expect(
    clippy::needless_bitwise_bool,
    reason = "`&` keeps the per-byte test branch-free so the fold vectorises; every operand is a cheap comparison"
)]
fn emitted(b: u8) -> bool {
    b.is_ascii_graphic() & (b != b'#') & (b != b'"') & (b != b'<') & (b != b'>')
}

/// The path (before the first `?`): WHATWG turns `\` into `/` and percent-encodes `^`, `` ` ``, `{` and `}` there
/// (path percent-encode set).
fn allowed_in_path(b: u8) -> bool {
    emitted(b) && !matches!(b, b'\\' | b'^' | b'`' | b'{' | b'}')
}

/// The query: WHATWG percent-encodes `'` in the query of special URLs.
fn allowed_in_query(b: u8) -> bool {
    emitted(b) && b != b'\''
}

/// The per-component check, for the rare target that holds a component-specific byte.
fn components_allowed(target: &[u8]) -> bool {
    let mut components = target.splitn(2, |&b| b == b'?');
    let path = components.next().unwrap_or_default();
    let query = components.next().unwrap_or_default();
    path.iter().all(|&b| allowed_in_path(b)) && query.iter().all(|&b| allowed_in_query(b))
}

#[cfg(test)]
mod tests {
    use super::{allowed_everywhere, allowed_in_path, allowed_in_query};

    /// Every byte against the rule written out byte by byte: (everywhere, in the path, in the query).
    #[test]
    fn byte_classes_follow_the_whatwg_percent_encode_sets() {
        for b in 0..=u8::MAX {
            let expected = match b {
                b'#' | b'"' | b'<' | b'>' => (false, false, false),
                b'\\' | b'^' | b'`' | b'{' | b'}' => (false, false, true),
                b'\'' => (false, true, false),
                _ if b.is_ascii_graphic() => (true, true, true),
                _ => (false, false, false),
            };
            let actual = (
                allowed_everywhere(b),
                allowed_in_path(b),
                allowed_in_query(b),
            );
            assert_eq!(actual, expected, "{b:#04x}");
        }
    }
}
