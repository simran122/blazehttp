//! Listeners and accept loops: one task per connection on the caller's runtime (design note §4, design A).

use std::io;
use std::net::SocketAddr;

use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::pki_types::CertificateDer;

use crate::cells::prebuild;
use crate::connection::{self, Mode};
use crate::tls;

/// Where to listen and how to answer.
#[derive(Clone, Copy, Debug)]
pub struct ServerConfig {
    /// Plain HTTP/1.1 address (port 0 picks a free port; the bound address is reported in [`Bound`]).
    pub plain: SocketAddr,
    /// HTTPS address serving the same cells, if any.
    pub tls: Option<SocketAddr>,
    /// Accept and read, never answer.
    pub stall: bool,
}

/// The addresses actually bound, sent once the server accepts connections.
#[derive(Debug)]
pub struct Bound {
    pub plain: SocketAddr,
    pub tls: Option<TlsBound>,
}

#[derive(Debug)]
pub struct TlsBound {
    pub addr: SocketAddr,
    /// The self-signed certificate generated at start-up, for clients that verify it.
    pub certificate: CertificateDer<'static>,
}

struct TlsPort {
    listener: TcpListener,
    acceptor: TlsAcceptor,
    certificate: CertificateDer<'static>,
}

/// Binds, builds every response, reports [`Bound`] on `ready`, then serves until an accept fails.
///
/// # Errors
///
/// Binding a listener, generating the TLS identity, or a non-transient accept error.
pub async fn serve(cfg: ServerConfig, ready: oneshot::Sender<Bound>) -> io::Result<()> {
    let mode = if cfg.stall { Mode::Stall } else { Mode::Answer };
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
    tokio::try_join!(accept_plain(plain, mode), accept_tls(tls, mode))?;
    Ok(())
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
            certificate: self.certificate.clone(),
        })
    }
}

async fn accept_plain(listener: TcpListener, mode: Mode) -> io::Result<()> {
    loop {
        let stream = accept(&listener).await?;
        tokio::spawn(connection::run(stream, mode));
    }
}

async fn accept_tls(port: Option<TlsPort>, mode: Mode) -> io::Result<()> {
    let Some(TlsPort {
        listener, acceptor, ..
    }) = port
    else {
        return Ok(());
    };
    loop {
        let stream = accept(&listener).await?;
        let acceptor = acceptor.clone();
        tokio::spawn(async move { connection::run(acceptor.accept(stream).await?, mode).await });
    }
}

/// The next connection with `TCP_NODELAY` set (responses must not wait for Nagle); skips connections that died in
/// the accept queue.
async fn accept(listener: &TcpListener) -> io::Result<TcpStream> {
    loop {
        match listener.accept().await {
            Ok((stream, _)) if stream.set_nodelay(true).is_ok() => return Ok(stream),
            Ok(_) => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::ConnectionAborted | io::ErrorKind::ConnectionReset
                ) => {}
            Err(e) => return Err(e),
        }
    }
}
