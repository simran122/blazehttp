//! The TLS identity: a self-signed certificate generated at start-up (rcgen) and a rustls acceptor pinned to one
//! cipher suite and one key exchange, so every benchmark client negotiates the same work (design note §3).

use std::io;
use std::sync::Arc;

use rcgen::{CertifiedKey, generate_simple_self_signed};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls::crypto::{CryptoProvider, ring};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use tokio_rustls::rustls::{ServerConfig, version};

/// Names the certificate is valid for: clients connect through the loopback interface.
const SUBJECT_ALT_NAMES: [&str; 2] = ["localhost", "127.0.0.1"];

/// The certificate a TLS port presents, in both encodings clients load.
#[derive(Clone, Debug)]
pub(crate) struct Certificate {
    pub(crate) der: CertificateDer<'static>,
    pub(crate) pem: String,
}

/// The crypto every TLS benchmark cell uses: ring, TLS 1.3 `TLS13_AES_128_GCM_SHA256`, X25519 only.
///
/// Rust clients in the harness (and the tests) use the same provider, so no cell compares different cipher suites.
#[must_use]
pub fn benchmark_crypto_provider() -> Arc<CryptoProvider> {
    Arc::new(CryptoProvider {
        cipher_suites: vec![ring::cipher_suite::TLS13_AES_128_GCM_SHA256],
        kx_groups: vec![ring::kx_group::X25519],
        ..ring::default_provider()
    })
}

/// A fresh key pair and certificate, and an acceptor that presents them with ALPN `http/1.1`.
pub(crate) fn identity() -> io::Result<(TlsAcceptor, Certificate)> {
    let names = SUBJECT_ALT_NAMES.map(String::from).to_vec();
    let CertifiedKey { cert, signing_key } =
        generate_simple_self_signed(names).map_err(io::Error::other)?;
    let key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());
    let mut config = ServerConfig::builder_with_provider(benchmark_crypto_provider())
        .with_protocol_versions(&[&version::TLS13])
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(vec![cert.der().clone()], key.into())
        .map_err(io::Error::other)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let certificate = Certificate {
        der: cert.der().clone(),
        pem: cert.pem(),
    };
    Ok((TlsAcceptor::from(Arc::new(config)), certificate))
}
