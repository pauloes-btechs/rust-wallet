//! TLS that trusts exactly one certificate: the node's own `tls.cert`.
//!
//! LND generates a self-signed certificate with `IsCA: true`. webpki rejects
//! a CA certificate presented as an end entity, so instead of loosening
//! validation we *pin*: the handshake succeeds only if the server presents
//! the byte-identical certificate we were given, and its handshake signature
//! verifies against that certificate's key. Hostname checks are unnecessary
//! under an exact pin.

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls12_signature, verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, Error, SignatureScheme};
use std::sync::Arc;

#[derive(Debug)]
pub(crate) struct PinnedCert {
    der: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

impl PinnedCert {
    pub(crate) fn new(der: Vec<u8>) -> Self {
        Self { der, provider: Arc::new(rustls::crypto::ring::default_provider()) }
    }
}

impl ServerCertVerifier for PinnedCert {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, Error> {
        if end_entity.as_ref() == self.der.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(Error::General("server certificate does not match the pinned tls.cert".into()))
        }
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// Client config for gRPC over HTTP/2 with the pinned verifier.
pub(crate) fn client_config(cert_der: Vec<u8>) -> rustls::ClientConfig {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut cfg = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2 and 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(PinnedCert::new(cert_der)))
        .with_no_client_auth();
    cfg.alpn_protocols = vec![b"h2".to_vec()];
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// A self-signed cert flagged as a CA, exactly like LND's tls.cert.
    fn lnd_like_cert() -> (Vec<u8>, Vec<u8>) {
        let mut p = rcgen::CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()]).unwrap();
        p.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().unwrap();
        let cert = p.self_signed(&key).unwrap();
        (cert.der().to_vec(), key.serialize_der())
    }

    async fn handshake(server: (Vec<u8>, Vec<u8>), pinned: Vec<u8>) -> std::io::Result<()> {
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let mut scfg = rustls::ServerConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(server.0)],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(server.1)),
            )
            .unwrap();
        scfg.alpn_protocols = vec![b"h2".to_vec()];
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(scfg));
        let srv = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await?;
            let mut s = acceptor.accept(tcp).await?;
            s.write_all(b"ok").await?;
            s.flush().await?;
            Ok::<_, std::io::Error>(())
        });
        let connector = tokio_rustls::TlsConnector::from(Arc::new(client_config(pinned)));
        let tcp = tokio::net::TcpStream::connect(addr).await?;
        let mut c = connector.connect(ServerName::try_from("localhost").unwrap().to_owned(), tcp).await?;
        let mut buf = [0u8; 2];
        c.read_exact(&mut buf).await?;
        assert_eq!(&buf, b"ok");
        assert_eq!(c.get_ref().1.alpn_protocol(), Some(&b"h2"[..]));
        let _ = srv.await;
        Ok(())
    }

    #[tokio::test]
    async fn accepts_exactly_the_pinned_ca_flagged_cert() {
        let server = lnd_like_cert();
        handshake(server.clone(), server.0.clone()).await.expect("pinned cert must be accepted");
    }

    #[tokio::test]
    async fn rejects_any_other_cert() {
        let server = lnd_like_cert();
        let other = lnd_like_cert();
        assert!(handshake(server, other.0).await.is_err());
    }
}
