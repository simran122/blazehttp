//! HTTP/1.1 protocol pieces of the engine (design §5). The public API is re-exported here; the modules stay private.
//! The lint block below applies to every module of the component (parser, writer, shared grammar): no indexing,
//! unchecked arithmetic, unwrap/expect or panics in protocol code.

#![deny(
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable
)]

pub(crate) mod grammar;
mod parser;
mod writer;

use crate::generated::spec::{EngineDetail, ErrorCode, TooLargeDetail};

/// A caller broke a component's contract (for the writer: the JS layer's validation was bypassed); never a peer's
/// fault. Shared by the parser and the writer.
const INTERNAL: ErrorCode = ErrorCode::Engine(EngineDetail::Internal);
/// A head (response) or the request fields exceed their byte limit.
const HEADERS_TOO_LARGE: ErrorCode = ErrorCode::TooLarge(TooLargeDetail::Headers);

pub use parser::{Framing, HttpVersion, ParseStatus, ResponseHead, ResponseParser};
pub use writer::{HeaderTemplate, MAX_P0_REQUEST_BYTES, RequestParts, Reuse, write_request};
