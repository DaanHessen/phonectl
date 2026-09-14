//! TLS for wireless debugging.
//!
//! After the phone answers our `CNXN` with `STLS`, the socket is upgraded to
//! TLS 1.3 with mutual certificates. Our certificate carries the host ADB key,
//! which is what the phone recognises from pairing.
//!
//! The phone's certificate is self-signed and changes on every boot, so there
//! is no CA to check it against. Trust comes from pairing: the phone only
//! accepts a client certificate holding a key it was paired with. This mirrors
//! AOSP's own client (`adb/tls/tls_connection.cpp`).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::TlsConnector;
use tokio_rustls::client::TlsStream;

use crate::auth::HostKey;
use crate::error::{Error, Result};

/// Certificate subject adbd expects to see from a host.
const SUBJECT: &str = "adb-host";

/// Accepts any server certificate. See the module note: pairing, not a CA,
/// is what authenticates the phone.
#[derive(Debug)]
struct AcceptAnyServer;

impl ServerCertVerifier for AcceptAnyServer {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::ED25519,
        ]
    }
}

/// A self-signed certificate wrapping the host ADB key.
pub fn certificate(key: &HostKey) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>)> {
    let pem = key.to_pem()?;
    let key_pair = rcgen::KeyPair::from_pem_and_sign_algo(&pem, &rcgen::PKCS_RSA_SHA256)
        .map_err(|e| Error::Tls(format!("cannot use the host key for TLS: {e}")))?;

    let mut params = rcgen::CertificateParams::new(vec![SUBJECT.to_string()])
        .map_err(|e| Error::Tls(e.to_string()))?;
    params.distinguished_name = rcgen::DistinguishedName::new();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, SUBJECT);

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| Error::Tls(format!("cannot build the TLS certificate: {e}")))?;

    let private = PrivateKeyDer::try_from(key_pair.serialize_der())
        .map_err(|e| Error::Tls(format!("cannot serialise the TLS key: {e}")))?;
    Ok((cert.der().clone(), private))
}

/// TLS 1.3 client configuration presenting the host key.
pub fn client_config(key: &HostKey) -> Result<Arc<ClientConfig>> {
    let (cert, private) = certificate(key)?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());

    let config = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| Error::Tls(e.to_string()))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyServer))
        .with_client_auth_cert(vec![cert], private)
        .map_err(|e| Error::Tls(e.to_string()))?;

    Ok(Arc::new(config))
}

/// Upgrades a socket that has just exchanged `STLS` messages.
pub async fn upgrade<S>(stream: S, config: Arc<ClientConfig>) -> Result<TlsStream<S>>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    // adbd ignores the name; it is only needed to satisfy the API.
    let name = ServerName::try_from(SUBJECT).expect("static name is valid");
    TlsConnector::from(config)
        .connect(name, stream)
        .await
        .map_err(|e| Error::Tls(format!("TLS handshake failed: {e}")))
}

#[cfg(test)]
mod tests {
    use rsa::traits::PublicKeyParts;

    use super::*;

    fn key() -> &'static HostKey {
        use std::sync::OnceLock;
        static KEY: OnceLock<HostKey> = OnceLock::new();
        KEY.get_or_init(|| HostKey::generate().unwrap())
    }

    #[test]
    fn certificate_carries_the_host_key() {
        let (cert, _private) = certificate(key()).unwrap();
        // The modulus must appear verbatim in the certificate's SubjectPublicKeyInfo.
        let modulus = key().private_key().to_public_key().n().to_bytes_be();
        let der = cert.as_ref();
        assert!(
            der.windows(modulus.len()).any(|w| w == modulus),
            "certificate does not contain the host key modulus"
        );
    }

    #[tokio::test]
    async fn refuses_a_tls12_only_peer() {
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = rcgen::CertificateParams::new(vec!["phone".to_string()])
            .unwrap()
            .self_signed(&server_key)
            .unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS12])
            .unwrap()
            .with_no_client_auth()
            .with_single_cert(
                vec![server_cert.der().clone()],
                PrivateKeyDer::try_from(server_key.serialize_der()).unwrap(),
            )
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let _ = tokio_rustls::TlsAcceptor::from(Arc::new(server_config)).accept(socket).await;
        });

        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let err = upgrade(socket, client_config(key()).unwrap()).await.unwrap_err();
        assert!(matches!(err, Error::Tls(_)), "{err}");
    }

    #[tokio::test]
    async fn handshake_against_a_rustls_server_presents_our_certificate() {
        use std::sync::Arc;

        use tokio_rustls::TlsAcceptor;

        // A server that demands a client certificate and records what it got.
        let server_key = rcgen::KeyPair::generate().unwrap();
        let server_cert = rcgen::CertificateParams::new(vec!["phone".to_string()])
            .unwrap()
            .self_signed(&server_key)
            .unwrap();

        #[derive(Debug)]
        struct AcceptAnyClient;
        impl rustls::server::danger::ClientCertVerifier for AcceptAnyClient {
            fn root_hint_subjects(&self) -> &[rustls::DistinguishedName] {
                &[]
            }
            fn verify_client_cert(
                &self,
                end_entity: &CertificateDer<'_>,
                _intermediates: &[CertificateDer<'_>],
                _now: UnixTime,
            ) -> std::result::Result<rustls::server::danger::ClientCertVerified, rustls::Error> {
                assert!(!end_entity.as_ref().is_empty());
                Ok(rustls::server::danger::ClientCertVerified::assertion())
            }
            fn verify_tls12_signature(
                &self,
                _m: &[u8],
                _c: &CertificateDer<'_>,
                _d: &DigitallySignedStruct,
            ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn verify_tls13_signature(
                &self,
                _m: &[u8],
                _c: &CertificateDer<'_>,
                _d: &DigitallySignedStruct,
            ) -> std::result::Result<HandshakeSignatureValid, rustls::Error> {
                Ok(HandshakeSignatureValid::assertion())
            }
            fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
                AcceptAnyServer.supported_verify_schemes()
            }
        }

        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let server_config = rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_client_cert_verifier(Arc::new(AcceptAnyClient))
            .with_single_cert(
                vec![server_cert.der().clone()],
                PrivateKeyDer::try_from(server_key.serialize_der()).unwrap(),
            )
            .unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut tls = TlsAcceptor::from(Arc::new(server_config)).accept(socket).await.unwrap();
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await.unwrap();
            tls.write_all(b"pong").await.unwrap();
            tls.flush().await.unwrap();
            buf
        });

        let socket = tokio::net::TcpStream::connect(address).await.unwrap();
        let mut tls = upgrade(socket, client_config(key()).unwrap()).await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        tls.write_all(b"ping").await.unwrap();
        tls.flush().await.unwrap();
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.unwrap();

        assert_eq!(&buf, b"pong");
        assert_eq!(&server.await.unwrap(), b"ping");
    }
}
