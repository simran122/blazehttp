//! HTTP/1.1 protocol pieces of the engine (design §5).

pub(crate) mod grammar;
pub mod parser;

pub use parser::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
