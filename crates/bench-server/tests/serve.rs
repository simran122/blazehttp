//! `bench-server` end to end: real sockets, real TLS, the committed `bench/cells.json`.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use bench_server::{Bound, ServerConfig, cells, cells_json, serve};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};

const ANY_PORT: &str = "127.0.0.1:0";

async fn start(tls: bool, stall: bool) -> Bound {
    let addr: SocketAddr = ANY_PORT.parse().expect("address");
    let cfg = ServerConfig {
        plain: addr,
        tls: tls.then_some(addr),
        stall,
    };
    let (ready, bound) = oneshot::channel();
    tokio::spawn(serve(cfg, ready));
    bound.await.expect("server ready")
}

fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nhost: bench\r\n\r\n").into_bytes()
}

async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &[u8],
    expected_len: usize,
) -> Vec<u8> {
    stream.write_all(request).await.expect("write");
    let mut response = vec![0; expected_len];
    stream.read_exact(&mut response).await.expect("read");
    response
}

/// Every cell, then an unknown path, on one connection: the exact pre-built bytes, and a 404.
async fn check_every_cell<S: AsyncRead + AsyncWrite + Unpin>(stream: &mut S) {
    for cell in cells() {
        let response = exchange(stream, &get(cell.path), cell.response.len()).await;
        assert!(response == cell.response, "{}: bytes differ", cell.path);
    }
    let not_found = b"HTTP/1.1 404 Not Found\r\n";
    let response = exchange(stream, &get("/nope"), not_found.len()).await;
    assert_eq!(response, not_found);
}

#[tokio::test(flavor = "multi_thread")]
async fn each_cell_returns_prebuilt_bytes() {
    let bound = start(false, false).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
    check_every_cell(&mut stream).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn keep_alive_serves_10k_requests_on_one_connection() {
    let bound = start(false, false).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
    stream.set_nodelay(true).expect("nodelay");
    let cell = &cells()[0];
    let request = get(cell.path);
    for i in 0..10_000 {
        let response = exchange(&mut stream, &request, cell.response.len()).await;
        assert!(response == cell.response, "request {i}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn pipelined_requests_and_request_bodies_are_answered_in_order() {
    let bound = start(false, false).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
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
    let bound = start(false, false).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
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
async fn chunked_request_body_closes_the_connection() {
    let bound = start(false, false).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
    let request = b"POST /1k HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n5\r\nhello\r\n0\r\n\r\n";
    stream.write_all(request).await.expect("write");
    let mut rest = Vec::new();
    stream.read_to_end(&mut rest).await.expect("read to EOF");
    assert!(rest.is_empty(), "no response expected");
}

#[tokio::test(flavor = "multi_thread")]
async fn tls_port_serves_same_cells() {
    let bound = start(true, false).await;
    let tls = bound.tls.expect("TLS port bound");
    let mut roots = RootCertStore::empty();
    roots
        .add(tls.certificate)
        .expect("self-signed certificate as a root");
    let config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tcp = TcpStream::connect(tls.addr).await.expect("connect");
    let name = ServerName::try_from("localhost").expect("server name");
    let mut stream = TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .expect("handshake");
    check_every_cell(&mut stream).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stall_mode_never_answers() {
    let bound = start(false, true).await;
    let mut stream = TcpStream::connect(bound.plain).await.expect("connect");
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
