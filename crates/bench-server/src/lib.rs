//! `bench-server`: the fixed-response HTTP/1.1 server every benchmark cell runs against (design §16.2,
//! `docs/notes/bench-server.md`). Every response is pre-built at start-up; a request costs one scan for the blank
//! line, a 5-entry lookup and one write, so the server never becomes the bottleneck.

mod accept;
mod cells;
mod connection;
mod manifest;
mod request;
mod server;
mod tls;

pub use cells::{Cell, cells};
pub use manifest::cells_json;
pub use server::{Bound, ServerConfig, TlsBound, serve};
pub use tls::benchmark_crypto_provider;
