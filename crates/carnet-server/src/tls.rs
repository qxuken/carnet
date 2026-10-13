//! TLS on the server's own listener, for when nothing on the machine ends
//! it in front: Cloudflare proxying to the server itself, with an Origin
//! certificate (README: "Cloudflare proxied, with an Origin
//! certificate"). Off by default: a proxy in front ends TLS, and the
//! server speaks plain HTTP behind it.
//!
//! rustls with ring, as the rest of Carnet's TLS: no OpenSSL. HTTP/1.1
//! alone (ALPN `http/1.1`), as the server speaks, the sync socket's
//! upgrade and all.
//!
//! With a client CA, a connection must show a certificate that CA signed,
//! or its handshake fails and nothing of it reaches the server:
//! Cloudflare's Authenticated Origin Pulls, so only Cloudflare reaches it
//! even by its address.

use std::fmt;
use std::io;
use std::path::Path;
use std::sync::Arc;

use tokio_rustls::rustls::pki_types::pem::{self, PemObject};
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio_rustls::rustls::server::WebPkiClientVerifier;
use tokio_rustls::rustls::{self, RootCertStore, ServerConfig};

/// The listener's certificate and key, and the CA its clients' must be
/// signed by, if any: what [`crate::server::Config::tls`] holds.
#[derive(Clone)]
pub struct Tls {
    pub(crate) acceptor: tokio_rustls::TlsAcceptor,
    clients: bool,
}

impl fmt::Debug for Tls {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tls").field("client_certificates", &self.clients).finish_non_exhaustive()
    }
}

impl Tls {
    /// From PEM files: `cert` (the chain, the server's own first), `key`
    /// (PKCS#8, PKCS#1 or SEC1), and `client_ca` (one or more CA
    /// certificates) to ask every client for a certificate one of them
    /// signed.
    pub fn load(cert: &Path, key: &Path, client_ca: Option<&Path>) -> io::Result<Self> {
        let bad = |what: &Path, e: &dyn fmt::Display| io::Error::new(io::ErrorKind::InvalidData, format!("{}: {e}", what.display()));
        let certs = pems(cert).map_err(|e| bad(cert, &e))?;
        let key_der = PrivateKeyDer::from_pem_file(key).map_err(|e| match e {
            pem::Error::NoItemsFound => bad(key, &"no private key in it"),
            e => bad(key, &e),
        })?;
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let builder = ServerConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().map_err(io::Error::other)?;
        let builder = match client_ca {
            Some(ca) => {
                let mut roots = RootCertStore::empty();
                for c in pems(ca).map_err(|e| bad(ca, &e))? {
                    roots.add(c).map_err(|e| bad(ca, &e))?;
                }
                let verifier = WebPkiClientVerifier::builder_with_provider(Arc::new(roots), provider).build().map_err(|e| bad(ca, &e))?;
                builder.with_client_cert_verifier(verifier)
            }
            None => builder.with_no_client_auth(),
        };
        let mut config = builder.with_single_cert(certs, key_der).map_err(|e| match e {
            rustls::Error::InconsistentKeys(_) => bad(key, &format_args!("not the key of the certificate in {}", cert.display())),
            e => bad(cert, &e),
        })?;
        config.alpn_protocols = vec![b"http/1.1".to_vec()];
        Ok(Tls { acceptor: tokio_rustls::TlsAcceptor::from(Arc::new(config)), clients: client_ca.is_some() })
    }

    /// Whether every client must show a certificate.
    pub fn asks_clients(&self) -> bool {
        self.clients
    }
}

/// Every certificate in a PEM file; none is an error.
fn pems(path: &Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs = CertificateDer::pem_file_iter(path).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    if certs.is_empty() {
        return Err("no certificate in it".into());
    }
    Ok(certs)
}
