//! HTTP/1.1 behaviour beyond the cells: persistence, `HEAD`, leading empty lines, refusals and graceful close.

use bench_server::cells;

use crate::support::{config, connect, exchange, get, start, until_close};

/// Status line and `connection: close` of a refusal (the rest is the fixed `content-length`/`date` head).
fn assert_refusal(response: &[u8], status: &str) {
    let text = String::from_utf8_lossy(response);
    assert!(
        text.starts_with(&format!("HTTP/1.1 {status}\r\n")),
        "{text}"
    );
    assert!(text.contains("\r\nconnection: close\r\n"), "{text}");
}

/// Pre-built bytes of the first cell.
fn first() -> &'static [u8] {
    &cells()[0].response
}

#[tokio::test(flavor = "multi_thread")]
async fn connection_close_ends_after_the_response() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let request = format!(
        "GET {} HTTP/1.1\r\nConnection: close\r\n\r\n",
        cells()[0].path
    );
    assert_eq!(until_close(&mut stream, request.as_bytes()).await, first());
}

#[tokio::test(flavor = "multi_thread")]
async fn http10_closes_unless_keep_alive() {
    let server = start(config(false)).await;
    let path = cells()[0].path;
    let mut kept = connect(server.bound.plain).await;
    let keep_alive = format!("GET {path} HTTP/1.0\r\nconnection: keep-alive\r\n\r\n");
    for _ in 0..2 {
        assert_eq!(
            exchange(&mut kept, keep_alive.as_bytes(), first().len()).await,
            first()
        );
    }
    let mut closed = connect(server.bound.plain).await;
    let plain_10 = format!("GET {path} HTTP/1.0\r\n\r\n");
    assert_eq!(until_close(&mut closed, plain_10.as_bytes()).await, first());
}

#[tokio::test(flavor = "multi_thread")]
async fn head_returns_only_the_head_and_keeps_the_connection() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    for cell in cells() {
        let request = format!("HEAD {} HTTP/1.1\r\n\r\n", cell.path);
        let head = exchange(&mut stream, request.as_bytes(), cell.head_len).await;
        assert!(head == cell.response[..cell.head_len], "{}", cell.path);
    }
    assert_eq!(
        exchange(&mut stream, &get(cells()[0].path), first().len()).await,
        first()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn leading_empty_lines_are_skipped() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let request = [b"\r\n\r\n".as_slice(), &get(cells()[0].path)].concat();
    assert_eq!(
        exchange(&mut stream, &request, first().len()).await,
        first()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_field_lines_get_400_and_close() {
    let server = start(config(false)).await;
    for request in [
        "GET /1k HTTP/1.1\r\nhost : bench\r\n\r\n",
        "GET /1k HTTP/1.1\r\nhost: bench\r\n folded\r\n\r\n",
        "POST /1k HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n",
    ] {
        let mut stream = connect(server.bound.plain).await;
        assert_refusal(
            &until_close(&mut stream, request.as_bytes()).await,
            "400 Bad Request",
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_head_gets_431_and_close() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let request = format!("GET /1k HTTP/1.1\r\nx-big: {}\r\n\r\n", "a".repeat(20_000));
    assert_refusal(
        &until_close(&mut stream, request.as_bytes()).await,
        "431 Request Header Fields Too Large",
    );
}

/// Unread bytes after a refusal must not turn the close into a reset that destroys the responses before it.
#[tokio::test(flavor = "multi_thread")]
async fn responses_before_a_refusal_survive_unread_input() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let mut request = [
        get(cells()[0].path),
        get(cells()[1].path),
        b"BROKEN\r\n\r\n".to_vec(),
    ]
    .concat();
    request.extend(std::iter::repeat_n(b'x', 50_000));
    let response = until_close(&mut stream, &request).await;
    let expected = [first(), &cells()[1].response].concat();
    assert!(response.starts_with(&expected), "earlier responses lost");
    assert_refusal(&response[expected.len()..], "400 Bad Request");
}
