//! What a head's fields mean for framing and reuse (design §5.2; `docs/notes/response-parser.md` §6.5): the
//! `Content-Length`, `Transfer-Encoding` and `Connection` facts, folded as each validated field line completes, and
//! the final head's `Framing` and keep-alive.

#![deny(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]

use super::{Framing, HttpVersion, MALFORMED, SMUGGLING};
use crate::generated::spec::ErrorCode;

/// The framing announced by the current head's fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FramingFact {
    None,
    Length(u64),
    Chunked,
}

/// What `Connection` options said; `Close` is sticky for the exchange (RFC 9112 §9.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConnectionOption {
    Default,
    KeepAlive,
    Close,
}

/// The status line of the head whose fields are being folded.
#[derive(Clone, Copy, Debug)]
pub(super) struct StatusLine {
    pub(super) status: u16,
    pub(super) version: HttpVersion,
}

impl StatusLine {
    pub(super) fn is_interim(self) -> bool {
        (100..200).contains(&self.status)
    }
}

/// The facts of one exchange: `framing` per head, `connection` across the interim heads and the final head.
#[derive(Clone, Copy, Debug)]
pub(super) struct Facts {
    framing: FramingFact,
    connection: ConnectionOption,
}

/// `1*DIGIT` within `u64` (RFC 9110 §8.6), or `None`.
fn decimal(value: &[u8]) -> Option<u64> {
    if value.is_empty() {
        return None;
    }
    value.iter().try_fold(0_u64, |n, &b| {
        if b.is_ascii_digit() {
            n.checked_mul(10)?
                .checked_add(u64::from(b.wrapping_sub(b'0')))
        } else {
            None
        }
    })
}

impl Facts {
    pub(super) const NEW: Self = Self {
        framing: FramingFact::None,
        connection: ConnectionOption::Default,
    };

    /// Folds one validated field line (`raw` is the value without CRLF). Only the framing names are trimmed: after
    /// validation the only ASCII whitespace left is SP/HTAB, so `trim_ascii` is exactly OWS trimming (obs-text is never
    /// trimmed). Names are matched case-insensitively (RFC 9110 §5.1), dispatched on length first.
    pub(super) fn field(
        &mut self,
        line: StatusLine,
        name: &[u8],
        raw: &[u8],
    ) -> Result<(), ErrorCode> {
        match name.len() {
            14 if name.eq_ignore_ascii_case(b"content-length") => {
                self.content_length(line, raw.trim_ascii())
            }
            17 if name.eq_ignore_ascii_case(b"transfer-encoding") => {
                self.transfer_encoding(line, raw.trim_ascii())
            }
            10 if name.eq_ignore_ascii_case(b"connection") => {
                self.connection_options(raw);
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn content_length(&mut self, line: StatusLine, value: &[u8]) -> Result<(), ErrorCode> {
        if self.framing != FramingFact::None {
            return Err(SMUGGLING);
        }
        let length = decimal(value).ok_or(MALFORMED)?;
        if line.is_interim() && length != 0 {
            return Err(SMUGGLING);
        }
        self.framing = FramingFact::Length(length);
        Ok(())
    }

    fn transfer_encoding(&mut self, line: StatusLine, value: &[u8]) -> Result<(), ErrorCode> {
        if self.framing != FramingFact::None
            || line.version == HttpVersion::Http10
            || line.is_interim()
            || !value.eq_ignore_ascii_case(b"chunked")
        {
            return Err(SMUGGLING);
        }
        self.framing = FramingFact::Chunked;
        Ok(())
    }

    fn connection_options(&mut self, raw: &[u8]) {
        for option in raw.split(|&b| b == b',').map(<[u8]>::trim_ascii) {
            if option.eq_ignore_ascii_case(b"close") {
                self.connection = ConnectionOption::Close;
            } else if option.eq_ignore_ascii_case(b"keep-alive")
                && self.connection != ConnectionOption::Close
            {
                self.connection = ConnectionOption::KeepAlive;
            }
        }
    }

    /// The facts carried past an interim head: only a `Connection: close` survives.
    pub(super) fn after_interim(self) -> Self {
        let connection = match self.connection {
            ConnectionOption::Close => ConnectionOption::Close,
            ConnectionOption::Default | ConnectionOption::KeepAlive => ConnectionOption::Default,
        };
        Self {
            connection,
            ..Self::NEW
        }
    }

    /// The final head's framing and whether its connection may be reused.
    pub(super) fn conclude(self, line: StatusLine, head_request: bool) -> (Framing, bool) {
        let bodiless_status = matches!(line.status, 204 | 304);
        let framing = match self.framing {
            _ if head_request || bodiless_status => Framing::Empty,
            FramingFact::Chunked => Framing::Chunked,
            FramingFact::Length(n) => Framing::ContentLength(n),
            FramingFact::None => Framing::CloseDelimited,
        };
        let persistent = match line.version {
            HttpVersion::Http11 => self.connection != ConnectionOption::Close,
            HttpVersion::Http10 => self.connection == ConnectionOption::KeepAlive,
        };
        // A 204/304 that announces a body is accepted without one, but the server may still send it: not reused.
        let announced_body = match self.framing {
            FramingFact::Chunked => true,
            FramingFact::Length(n) => n > 0,
            FramingFact::None => false,
        };
        let unsent_body = !head_request && bodiless_status && announced_body;
        let keep_alive = persistent && framing != Framing::CloseDelimited && !unsent_body;
        (framing, keep_alive)
    }
}

#[cfg(test)]
mod tests {
    use super::decimal;

    #[test]
    fn decimal_is_digits_within_u64() {
        assert_eq!(decimal(b"0"), Some(0));
        assert_eq!(decimal(b"007"), Some(7));
        assert_eq!(decimal(b"18446744073709551615"), Some(u64::MAX));
        for bad in [
            &b""[..],
            b"18446744073709551616",
            b"+1",
            b"-1",
            b"1 2",
            b"0x1",
            b"1,1",
        ] {
            assert_eq!(decimal(bad), None, "{bad:?}");
        }
    }
}
