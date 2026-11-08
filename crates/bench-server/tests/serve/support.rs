//! Shared helpers for the `bench-server` integration tests: a server on free loopback ports, raw HTTP/1.1 exchanges
//! and a TLS client with the server's own crypto provider.

use std::io;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bench_server::{Bound, ServerConfig, TlsBound, benchmark_crypto_provider, serve};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::timeout;
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore, version};

/// A running server: its addresses, the switch that stops it, and its task.
pub struct Running {
    pub bound: Bound,
    pub stop: oneshot::Sender<()>,
    pub task: JoinHandle<io::Result<()>>,
}

/// Loopback with a free port.
pub fn any_port() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

/// The default configuration on free loopback ports (TLS when asked).
pub fn config(tls: bool) -> ServerConfig {
    ServerConfig {
        plain: any_port(),
        tls: tls.then(any_port),
        ..ServerConfig::default()
    }
}

pub async fn start(cfg: ServerConfig) -> Running {
    let (ready, bound) = oneshot::channel();
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(serve(cfg, ready, async {
        let _ = stopped.await;
    }));
    Running {
        bound: bound.await.expect("server ready"),
        stop,
        task,
    }
}

/// A plain connection with `TCP_NODELAY`, so small test writes are not delayed by Nagle's algorithm.
pub async fn connect(addr: SocketAddr) -> TcpStream {
    let stream = TcpStream::connect(addr).await.expect("connect");
    stream.set_nodelay(true).expect("nodelay");
    stream
}

pub async fn connect_tls(tls: &TlsBound) -> TlsStream<TcpStream> {
    let mut roots = RootCertStore::empty();
    roots
        .add(tls.certificate.clone())
        .expect("self-signed certificate as a root");
    let config = ClientConfig::builder_with_provider(benchmark_crypto_provider())
        .with_protocol_versions(&[&version::TLS13])
        .expect("TLS 1.3")
        .with_root_certificates(roots)
        .with_no_client_auth();
    let name = ServerName::try_from("localhost").expect("server name");
    let tcp = connect(tls.addr).await;
    TlsConnector::from(Arc::new(config))
        .connect(name, tcp)
        .await
        .expect("handshake")
}

pub fn get(path: &str) -> Vec<u8> {
    format!("GET {path} HTTP/1.1\r\nhost: bench\r\n\r\n").into_bytes()
}

/// Writes `request` and reads exactly `expected_len` response bytes.
pub async fn exchange<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &[u8],
    expected_len: usize,
) -> Vec<u8> {
    stream.write_all(request).await.expect("write");
    let mut response = vec![0; expected_len];
    stream.read_exact(&mut response).await.expect("read");
    response
}

/// How long a test waits for the server to close a connection or stop: a server that never does fails the test.
pub const CLOSE_WITHIN: Duration = Duration::from_secs(5);

/// Writes `request` and reads until the server closes the connection; a reset, or no close within
/// [`CLOSE_WITHIN`], is a test failure.
pub async fn until_close<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    request: &[u8],
) -> Vec<u8> {
    stream.write_all(request).await.expect("write");
    timeout(CLOSE_WITHIN, read_to_close(stream))
        .await
        .expect("server closed the connection in time")
        .expect("read to a clean end of stream")
}

/// The most a test reads before end of stream: a server that never stops writing fails the test here, quickly and in
/// bounded memory, instead of growing a buffer until the machine runs out of memory.
const READ_CAP: usize = 16 << 20;

/// Reads until end of stream, at most `READ_CAP` bytes (more is a test failure).
pub async fn read_to_close<S: AsyncRead + Unpin>(stream: &mut S) -> io::Result<Vec<u8>> {
    let mut response = Vec::new();
    let limit = u64::try_from(READ_CAP + 1).expect("cap fits u64");
    (&mut *stream)
        .take(limit)
        .read_to_end(&mut response)
        .await?;
    assert!(
        response.len() <= READ_CAP,
        "read cap of {READ_CAP} bytes exceeded"
    );
    Ok(response)
}
