//! The TLS identity: a self-signed certificate generated at start-up (rcgen) and a rustls acceptor (ring provider).

use std::io;
use std::sync::Arc;

use rcgen::{CertifiedKey, generate_simple_self_signed};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::ServerConfig;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};

/// Names the certificate is valid for: benchmark clients connect to the loopback or a container name aliased to it.
const SUBJECT_ALT_NAMES: [&str; 2] = ["localhost", "127.0.0.1"];

/// A fresh key pair and certificate, and an acceptor that presents them with ALPN `http/1.1`.
pub(crate) fn identity() -> io::Result<(TlsAcceptor, CertificateDer<'static>)> {
    let names = SUBJECT_ALT_NAMES.map(String::from).to_vec();
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(names).map_err(io::Error::other)?;
    let key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key.into())
        .map_err(io::Error::other)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok((TlsAcceptor::from(Arc::new(config)), cert.der().clone()))
}
