//! `bench-server` end to end: real sockets, real TLS, the committed `bench/cells.json`.
//! HTTP behaviour beyond the cells is in `serve/protocol.rs`, connection limits and lifecycle in `serve/limits.rs`.

mod limits;
mod protocol;
mod support;

use std::path::Path;
use std::time::Duration;

use bench_server::{ServerConfig, cells, cells_json};
use support::{config, connect, connect_tls, exchange, get, start};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The pre-built 404, compared in full.
const NOT_FOUND: &[u8] =
    b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\ndate: Fri, 06 Nov 2026 12:00:00 GMT\r\n\r\n";

/// Every cell, then an unknown path, on one connection: the exact pre-built bytes, and the 404.
async fn check_every_cell<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) {
    for cell in cells() {
        let response = exchange(stream, &get(cell.path), cell.response.len()).await;
        assert!(response == cell.response, "{}: bytes differ", cell.path);
    }
    assert_eq!(
        exchange(stream, &get("/nope"), NOT_FOUND.len()).await,
        NOT_FOUND
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cell_returns_prebuilt_bytes() {
    let server = start(config(false)).await;
    check_every_cell(&mut connect(server.bound.plain).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_serves_10k_requests_on_one_connection() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let cell = &cells()[0];
    let request = get(cell.path);
    for i in 0..10_000 {
        let response = exchange(&mut stream, &request, cell.response.len()).await;
        assert!(response == cell.response, "request {i}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pipelined_requests_and_request_bodies_are_answered_in_order() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let [first, second, ..] = cells() else {
        panic!("at least two cells")
    };
    let mut requests = format!(
        "POST {} HTTP/1.1\r\nContent-Length: 11\r\n\r\nhello world",
        first.path
    )
    .into_bytes();
    requests.extend(get(second.path));
    requests.extend(get(first.path));
    let expected = [&first.response[..], &second.response, &first.response].concat();
    let response = exchange(&mut stream, &requests, expected.len()).await;
    assert!(response == expected, "pipelined responses differ");
}

#[tokio::test(flavor = "multi_thread")]
async fn byte_at_a_time_request_is_answered() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let cell = &cells()[0];
    let request = get(cell.path);
    let (last, head) = request.split_last().expect("non-empty");
    for byte in head {
        stream
            .write_all(std::slice::from_ref(byte))
            .await
            .expect("write");
    }
    let response = exchange(&mut stream, &[*last], cell.response.len()).await;
    assert!(response == cell.response, "bytes differ");
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_port_serves_same_cells() {
    let server = start(config(true)).await;
    let tls = server.bound.tls.as_ref().expect("TLS port bound");
    check_every_cell(&mut connect_tls(tls).await).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stall_mode_never_answers() {
    let server = start(ServerConfig {
        stall: true,
        ..config(false)
    })
    .await;
    let mut stream = connect(server.bound.plain).await;
    stream
        .write_all(&get(cells()[0].path))
        .await
        .expect("write");
    let mut byte = [0; 1];
    let read = tokio::time::timeout(Duration::from_millis(300), stream.read(&mut byte)).await;
    assert!(
        read.is_err(),
        "expected no bytes within 300 ms, got {read:?}"
    );
}

#[test]
fn cells_json_matches_cells() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bench/cells.json");
    let committed = std::fs::read_to_string(&path).expect("bench/cells.json");
    assert_eq!(
        committed,
        cells_json(),
        "regenerate with `bench-server --print-cells > bench/cells.json`"
    );
}
