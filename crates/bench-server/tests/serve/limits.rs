//! Connection limits and lifecycle: the connection cap, the head and handshake timeouts, stopping the server, and
//! the binary's `--cert-out`.

use std::process::Stdio;
use std::time::Duration;

use bench_server::{ServerConfig, cells};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time::timeout;

use crate::support::{
    CLOSE_WITHIN, config, connect, exchange, get, read_to_close, start, until_close,
};

const SHORT: Duration = Duration::from_millis(300);

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
    second
        .read_exact(&mut response)
        .await
        .expect("served once a slot is free");
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
    let closed = timeout(
        SHORT * 10,
        until_close(&mut stream, b"GET /1k HTTP/1.1\r\nhost:"),
    )
    .await;
    assert_eq!(closed.expect("closed by the head timeout"), b"");
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
    let mut open = connect(server.bound.plain).await;
    server.stop.send(()).expect("server running");
    timeout(CLOSE_WITHIN, server.task)
        .await
        .expect("server stopped in time")
        .expect("join")
        .expect("clean stop");
    let ended = timeout(SHORT * 10, read_to_close(&mut open))
        .await
        .expect("connection ended");
    assert!(
        ended.as_ref().map_or(true, Vec::is_empty),
        "connection still answered: {ended:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn binary_writes_the_certificate_before_ready() {
    let dir = std::env::temp_dir().join(format!("bench-server-cert-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let pem = dir.join("cert.pem");
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
    std::fs::remove_dir_all(&dir).expect("clean up");
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
