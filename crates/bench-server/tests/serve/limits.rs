//! Connection limits and lifecycle: the connection cap, the head and handshake timeouts, stopping the server, and
//! the binary's `--cert-out`.

use std::process::Stdio;

use bench_server::{ServerConfig, cells};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::support::{
    CLOSE_WITHIN, SHORT, config, connect, exchange, get, read_to_close, start, until_close,
};

#[tokio::test(flavor = "multi_thread")]
async fn connections_past_the_cap_wait_until_one_closes() {
    let server = start(ServerConfig {
        max_connections: 1,
        ..config(false)
    })
    .await;
    let cell = &cells()[0];
    let mut first = connect(server.bound.plain).await;
    assert_eq!(
        exchange(&mut first, &get(cell.path), cell.response.len()).await,
        cell.response
    );
    let mut second = connect(server.bound.plain).await;
    second.write_all(&get(cell.path)).await.expect("write");
    let mut byte = [0; 1];
    assert!(
        timeout(SHORT, second.read(&mut byte)).await.is_err(),
        "served past the cap"
    );
    drop(first);
    let mut response = vec![0; cell.response.len()];
    timeout(CLOSE_WITHIN, second.read_exact(&mut response))
        .await
        .expect("served once a slot is free")
        .expect("read");
    assert_eq!(response, cell.response);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unfinished_head_is_closed_after_the_request_timeout() {
    let server = start(ServerConfig {
        request_timeout: SHORT,
        ..config(false)
    })
    .await;
    let mut stream = connect(server.bound.plain).await;
    // `until_close` fails the test if the server has not closed within its own bound.
    assert_eq!(
        until_close(&mut stream, b"GET /1k HTTP/1.1\r\nhost:").await,
        b""
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_idle_connection_outlives_the_request_timeout() {
    let server = start(ServerConfig {
        request_timeout: SHORT,
        ..config(false)
    })
    .await;
    let cell = &cells()[0];
    let mut stream = connect(server.bound.plain).await;
    tokio::time::sleep(SHORT * 2).await;
    let response = exchange(&mut stream, &get(cell.path), cell.response.len()).await;
    assert_eq!(response, cell.response);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silent_tls_client_is_closed_after_the_handshake_timeout() {
    let server = start(ServerConfig {
        request_timeout: SHORT,
        ..config(true)
    })
    .await;
    let mut stream = connect(server.bound.tls.as_ref().expect("TLS port").addr).await;
    let closed = timeout(SHORT * 10, read_to_close(&mut stream)).await;
    assert!(closed.expect("closed by the handshake timeout").is_ok());
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_the_server_and_its_connections() {
    let server = start(config(false)).await;
    let cell = &cells()[0];
    let mut open = connect(server.bound.plain).await;
    // One exchange first, so a connection task really is running when the server stops.
    assert_eq!(
        exchange(&mut open, &get(cell.path), cell.response.len()).await,
        cell.response
    );
    server.stop.send(()).expect("server running");
    timeout(CLOSE_WITHIN, server.task)
        .await
        .expect("server stopped in time")
        .expect("join")
        .expect("clean stop");
    let rest = timeout(CLOSE_WITHIN, read_to_close(&mut open))
        .await
        .expect("connection ended");
    assert_eq!(rest.expect("ended without a reset"), b"");
}

/// A directory removed when the test ends, also when it fails.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new(prefix: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        // Best effort: a leftover temp dir must not turn a passing test into a panic during unwinding.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_writes_the_certificate_before_ready() {
    let dir = TempDir::new("bench-server-cert");
    let pem = dir.0.join("cert.pem");
    let mut child = Command::new(env!("CARGO_BIN_EXE_bench-server"))
        .args([
            "--plain",
            "127.0.0.1:0",
            "--tls",
            "127.0.0.1:0",
            "--threads",
            "1",
            "--cert-out",
        ])
        .arg(&pem)
        .stdout(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("spawn bench-server");
    let mut lines = BufReader::new(child.stdout.take().expect("stdout")).lines();
    let ready = timeout(SHORT * 20, lines.next_line())
        .await
        .expect("ready in time")
        .expect("stdout");
    assert!(
        ready.is_some_and(
            |line| line.starts_with("ready plain=127.0.0.1:") && line.contains(" tls=")
        )
    );
    let written = std::fs::read_to_string(&pem).expect("certificate written");
    assert!(
        written.starts_with("-----BEGIN CERTIFICATE-----"),
        "{written}"
    );
}

/// The test reader's own guard: more than 16 MiB before end of stream fails fast.
#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "read cap")]
async fn test_reader_fails_fast_past_its_cap() {
    let server = start(config(false)).await;
    let mut stream = connect(server.bound.plain).await;
    let mut requests = get("/1m").repeat(17);
    requests.extend_from_slice(b"GET /1k HTTP/1.1\r\nconnection: close\r\n\r\n");
    until_close(&mut stream, &requests).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn invalid_connection_limits_are_refused_before_binding() {
    for max_connections in [0, tokio::sync::Semaphore::MAX_PERMITS + 1] {
        let (ready, _bound) = tokio::sync::oneshot::channel();
        let cfg = ServerConfig {
            max_connections,
            ..config(false)
        };
        let served = timeout(
            CLOSE_WITHIN,
            bench_server::serve(cfg, ready, std::future::pending()),
        );
        let error = served.await.expect("refused at once").expect_err("refused");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::InvalidInput,
            "{max_connections}"
        );
    }
}

/// The helpers' own guard: a server that never answers fails `exchange` instead of hanging the test.
#[tokio::test(flavor = "multi_thread")]
#[should_panic(expected = "answered in time")]
async fn exchange_fails_fast_when_the_server_never_answers() {
    let server = start(ServerConfig {
        stall: true,
        ..config(false)
    })
    .await;
    let mut stream = connect(server.bound.plain).await;
    exchange(&mut stream, &get("/1k"), 10).await;
}
