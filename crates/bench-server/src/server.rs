//! Listeners and the server lifecycle: bind, report readiness, accept until stopped (design note §4, design A).
//!
//! Connections run as tasks on the caller's runtime. Run [`serve`] on a runtime of its own (as the binary does): a
//! benchmark client sharing its worker threads would measure the server's work as its own.

use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;
use tokio::sync::{Semaphore, oneshot};
use tokio::task::JoinSet;
use tokio::time::timeout;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::CertificateDer;

use crate::accept::accept_loop;
use crate::cells::prebuild;
use crate::connection::{self, Mode};
use crate::tls::{self, Certificate};

/// Where to listen and how to answer.
///
/// A passive configuration value, so its fields are public (design §13.2 rule 1 carve-out, Rust API Guidelines
/// C-STRUCT-PRIVATE). [`Default`] listens on `127.0.0.1:8080` only.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    /// Plain HTTP/1.1 address (port 0 picks a free port; the bound address is reported in [`Bound`]).
    pub plain: SocketAddr,
    /// HTTPS address serving the same cells, if any.
    pub tls: Option<SocketAddr>,
    /// Accept and read, never answer.
    pub stall: bool,
    /// Open connections per port; further clients wait in the listen backlog (G6).
    pub max_connections: usize,
    /// Time allowed for a request head once its first byte arrived, and for a TLS handshake (not in stall mode).
    pub head_timeout: Duration,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            plain: SocketAddr::from((Ipv4Addr::LOCALHOST, 8080)),
            tls: None,
            stall: false,
            max_connections: 1024,
            head_timeout: Duration::from_secs(10),
        }
    }
}

/// The addresses actually bound, sent once the server accepts connections (a passive value, public fields).
#[derive(Debug)]
pub struct Bound {
    pub plain: SocketAddr,
    pub tls: Option<TlsBound>,
}

/// The bound TLS port and its certificate (a passive value, public fields).
#[derive(Debug)]
pub struct TlsBound {
    pub addr: SocketAddr,
    /// The self-signed certificate generated at start-up, for clients that verify it.
    pub certificate: CertificateDer<'static>,
    /// The same certificate in PEM, for clients that load a file (`--cert-out`).
    pub certificate_pem: String,
}

struct TlsPort {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    certificate: Certificate,
}

/// Binds, builds every response, reports [`Bound`] on `ready`, then serves until `stop` completes or an accept
/// fails. On return every connection task has ended.
///
/// # Errors
///
/// Binding a listener, generating the TLS identity, or an accept error that is not transient.
pub async fn serve(
    cfg: ServerConfig,
    ready: oneshot::Sender<Bound>,
    stop: impl Future<Output = ()>,
) -> io::Result<()> {
    let plain = TcpListener::bind(cfg.plain).await?;
    let tls = match cfg.tls {
        Some(addr) => Some(bind_tls(addr).await?),
        None => None,
    };
    prebuild();
    let bound = Bound {
        plain: plain.local_addr()?,
        tls: tls.as_ref().map(TlsPort::bound).transpose()?,
    };
    // A caller that stopped waiting for readiness still gets a running server.
    let _ = ready.send(bound);
    let (mut plain_tasks, mut tls_tasks) = (JoinSet::new(), JoinSet::new());
    let result = tokio::select! {
        accepted = async {
            tokio::try_join!(
                serve_plain(&plain, &cfg, &mut plain_tasks),
                serve_tls(tls.as_ref(), &cfg, &mut tls_tasks),
            )
        } => accepted.map(|_| ()),
        () = stop => Ok(()),
    };
    plain_tasks.shutdown().await;
    tls_tasks.shutdown().await;
    result
}

async fn bind_tls(addr: SocketAddr) -> io::Result<TlsPort> {
    let listener = TcpListener::bind(addr).await?;
    let (acceptor, certificate) = tls::identity()?;
    Ok(TlsPort {
        listener,
        acceptor,
        certificate,
    })
}

impl TlsPort {
    fn bound(&self) -> io::Result<TlsBound> {
        Ok(TlsBound {
            addr: self.listener.local_addr()?,
            certificate: self.certificate.der.clone(),
            certificate_pem: self.certificate.pem.clone(),
        })
    }
}

impl ServerConfig {
    fn mode(&self) -> Mode {
        if self.stall {
            Mode::Stall
        } else {
            Mode::Answer {
                head_timeout: self.head_timeout,
            }
        }
    }

    fn connection_limit(&self) -> Arc<Semaphore> {
        Arc::new(Semaphore::new(self.max_connections))
    }
}

async fn serve_plain(
    listener: &TcpListener,
    cfg: &ServerConfig,
    tasks: &mut JoinSet<()>,
) -> io::Result<()> {
    let mode = cfg.mode();
    accept_loop(listener, &cfg.connection_limit(), tasks, move |stream| {
        connection::run(stream, mode)
    })
    .await
}

async fn serve_tls(
    port: Option<&TlsPort>,
    cfg: &ServerConfig,
    tasks: &mut JoinSet<()>,
) -> io::Result<()> {
    let Some(port) = port else {
        return Ok(());
    };
    let mode = cfg.mode();
    let handshake_timeout = match mode {
        Mode::Answer { head_timeout } => head_timeout,
        Mode::Stall => Duration::MAX,
    };
    let handle = |stream| {
        let accepting = port.acceptor.accept(stream);
        async move {
            let stream = timeout(handshake_timeout, accepting)
                .await
                .map_err(io::Error::from)??;
            connection::run(stream, mode).await
        }
    };
    accept_loop(&port.listener, &cfg.connection_limit(), tasks, handle).await
}
