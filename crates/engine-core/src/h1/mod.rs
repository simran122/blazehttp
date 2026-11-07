//! HTTP/1.1 protocol pieces of the engine (design §5). The public API is re-exported here; the modules stay private.

pub(crate) mod grammar;
mod parser;

pub use parser::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
