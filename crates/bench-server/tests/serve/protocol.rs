//! HTTP/1.1 behaviour beyond the cells: persistence, `HEAD`, versions, leading empty lines, refusals and graceful
//! close.

use std::sync::Arc;

use bench_server::cells;
use tokio_rustls::rustls::crypto::{CryptoProvider, ring};

use crate::support::{
    BAD_REQUEST, HEAD_TOO_LARGE, VERSION_NOT_SUPPORTED, config, connect, connect_tls, exchange,
    get, handshake, start, until_close,
};

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

/// A client that writes a whole large body before reading must get its response and a clean close, not a reset.
#[tokio::test(flavor = "multi_thread")]
async fn connection_close_after_a_large_body_is_answered_without_a_reset() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let body_len = 1 << 20;
    let head = format!(
        "POST {} HTTP/1.1\r\nconnection: close\r\ncontent-length: {body_len}\r\n\r\n",
        cells()[0].path
    );
    let mut request = head.into_bytes();
    request.resize(request.len() + body_len, b'x');
    assert_eq!(until_close(&mut stream, &request).await, first());
}

/// tokio-rustls reports a close without `close_notify` as an error, so a clean end of stream proves it was sent.
#[tokio::test(flavor = "multi_thread")]
async fn tls_close_sends_close_notify() {
    let server = start(config(true)).await;
    let mut stream = connect_tls(server.bound.tls.as_ref().expect("TLS port")).await;
    let request = format!(
        "GET {} HTTP/1.1\r\nconnection: close\r\n\r\n",
        cells()[0].path
    );
    assert_eq!(until_close(&mut stream, request.as_bytes()).await, first());
}

/// A client offering everything ring supports (preferring AES-256) still gets the one pinned suite and group.
#[tokio::test(flavor = "multi_thread")]
async fn tls_negotiates_only_the_pinned_suite_and_group() {
    let server = start(config(true)).await;
    let tls = server.bound.tls.as_ref().expect("TLS port");
    let stream = handshake(tls, Arc::new(ring::default_provider()))
        .await
        .expect("handshake");
    let (_, session) = stream.get_ref();
    let suite = session.negotiated_cipher_suite().expect("suite");
    assert_eq!(
        suite.suite(),
        ring::cipher_suite::TLS13_AES_128_GCM_SHA256.suite()
    );
    let group = session.negotiated_key_exchange_group().expect("group");
    assert_eq!(group.name(), ring::kx_group::X25519.name());
}

/// A client without X25519 shares no key exchange group with the server, so the handshake is refused.
#[tokio::test(flavor = "multi_thread")]
async fn tls_refuses_a_client_without_the_pinned_group() {
    let server = start(config(true)).await;
    let tls = server.bound.tls.as_ref().expect("TLS port");
    let without_x25519 = CryptoProvider {
        kx_groups: vec![ring::kx_group::SECP256R1, ring::kx_group::SECP384R1],
        ..ring::default_provider()
    };
    assert!(handshake(tls, Arc::new(without_x25519)).await.is_err());
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
async fn other_major_versions_get_505_and_close() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let request = "GET /1k HTTP/2.0\r\n\r\n";
    assert_eq!(
        until_close(&mut stream, request.as_bytes()).await,
        VERSION_NOT_SUPPORTED
    );
}

/// HTTP/1.0 cannot receive chunked framing (RFC 9112 §6.1), so the chunked cell's body comes close-delimited, and the
/// connection closes even when the client asked to keep it alive (the close is the body's only end).
#[tokio::test(flavor = "multi_thread")]
async fn http10_gets_the_chunked_cell_close_delimited() {
    let server = start(config(false)).await;
    let chunked = cells()
        .iter()
        .find(|cell| cell.path == "/1k-chunked")
        .expect("the chunked cell");
    let payload = &cells()[0].response[cells()[0].head_len..];
    for fields in ["", "connection: keep-alive\r\n"] {
        let mut stream = connect(server.bound.plain).await;
        let request = format!("GET {} HTTP/1.0\r\n{fields}\r\n", chunked.path);
        let response = until_close(&mut stream, request.as_bytes()).await;
        let text = String::from_utf8_lossy(&response);
        assert!(
            text.starts_with("HTTP/1.1 200 OK\r\n"),
            "{fields:?}: {text}"
        );
        assert!(
            !text.to_ascii_lowercase().contains("transfer-encoding"),
            "{fields:?}: {text}"
        );
        assert!(
            response.ends_with(payload),
            "{fields:?}: the 1 KiB payload, unframed"
        );
    }
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
        assert_eq!(
            until_close(&mut stream, request.as_bytes()).await,
            BAD_REQUEST,
            "{request}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_head_gets_431_and_close() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let request = format!("GET /1k HTTP/1.1\r\nx-big: {}\r\n\r\n", "a".repeat(20_000));
    assert_eq!(
        until_close(&mut stream, request.as_bytes()).await,
        HEAD_TOO_LARGE
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
    let expected = [first(), &cells()[1].response, BAD_REQUEST].concat();
    assert!(
        response == expected,
        "responses before the 400 lost or changed"
    );
}
