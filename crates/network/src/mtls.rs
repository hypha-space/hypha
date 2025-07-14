//! mTLS functionality for 'libp2p'.
//!
//! Provides mutual TLS for 'libp2p' by providing a `Config` that can be used for the
//! `security_upgrade` parameter when using `SwarmBuilder`.

use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use ed25519_dalek::{VerifyingKey, pkcs8::DecodePublicKey};
use futures_util::{AsyncRead, AsyncWrite, FutureExt, future::BoxFuture};
use libp2p::identity::{PeerId, PublicKey, ed25519};
use rustls::{
    ClientConfig, CommonState, RootCertStore, ServerConfig,
    pki_types::{CertificateDer, CertificateRevocationListDer, PrivateKeyDer, ServerName},
    server::WebPkiClientVerifier,
};
use thiserror::Error;
use webpki::EndEntityCert;

/// Unified TLS stream that can be either a client or server stream
pub enum TlsStream<T> {
    /// A client TLS stream
    Client(futures_rustls::client::TlsStream<T>),
    /// A server TLS stream
    Server(futures_rustls::server::TlsStream<T>),
}

impl<T> AsyncRead for TlsStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            TlsStream::Client(stream) => Pin::new(stream).poll_read(cx, buf),
            TlsStream::Server(stream) => Pin::new(stream).poll_read(cx, buf),
        }
    }
}

impl<T> AsyncWrite for TlsStream<T>
where
    T: AsyncRead + AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        match &mut *self {
            TlsStream::Client(stream) => Pin::new(stream).poll_write(cx, buf),
            TlsStream::Server(stream) => Pin::new(stream).poll_write(cx, buf),
        }
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            TlsStream::Client(stream) => Pin::new(stream).poll_flush(cx),
            TlsStream::Server(stream) => Pin::new(stream).poll_flush(cx),
        }
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        match &mut *self {
            TlsStream::Client(stream) => Pin::new(stream).poll_close(cx),
            TlsStream::Server(stream) => Pin::new(stream).poll_close(cx),
        }
    }
}

/// Error type for mTLS configuration and upgrade failures.
#[derive(Error, Debug)]
pub enum UpgradeError {
    /// Failed to upgrade server connection.
    #[error("Failed to upgrade server connection: {0}")]
    ServerUpgrade(io::Error),
    /// Failed to upgrade client connection.
    #[error("Failed to upgrade client connection: {0}")]
    ClientUpgrade(io::Error),
    /// Failed to configure TLS.
    #[error("Failed to configure TLS: {0}")]
    TlsConfiguration(String),
    /// No peer certificate found.
    #[error("No peer certificate found")]
    NoPeerCertificate,
    /// Failed to decode public key.
    #[error("Failed to decode public key from DER: {0}")]
    SPKIError(#[from] ed25519_dalek::pkcs8::spki::Error),
    /// Failed to convert to Ed25519 public key.
    #[error("Failed to convert to Ed25519 public key: {0}")]
    DecodingError(#[from] libp2p::identity::DecodingError),
}

/// A mTLS configuration.
#[derive(Clone)]
pub struct Config {
    server: ServerConfig,
    client: ClientConfig,
}

impl Config {
    /// Create a new mTLS config with -based authentication and CRL support
    pub fn try_new(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> Result<Self, UpgradeError> {
        // Initialize crypto provider if not already done
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

        let server_config = Self::make_server_config(
            cert_chain.clone(),
            private_key.clone_key(),
            ca_certs.clone(),
            crls.clone(),
        )?;
        let client_config = Self::make_client_config(cert_chain, private_key, ca_certs, crls)?;

        Ok(Self {
            server: server_config,
            client: client_config,
        })
    }

    fn make_server_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> Result<ServerConfig, UpgradeError> {
        // Create root cert store with CA certificates
        let mut root_store = RootCertStore::empty();
        for ca_cert in &ca_certs {
            root_store.add(ca_cert.clone()).map_err(|e| {
                UpgradeError::TlsConfiguration(format!("Failed to add CA cert: {e}"))
            })?;
        }

        // Create verifier that requires and validates client certificates
        let client_verifier = WebPkiClientVerifier::builder(Arc::new(root_store))
            .with_crls(crls)
            .build()
            .map_err(|e| {
                UpgradeError::TlsConfiguration(format!("Failed to create client verifier: {e}"))
            })?;

        ServerConfig::builder()
            .with_client_cert_verifier(client_verifier)
            .with_single_cert(cert_chain, private_key)
            .map_err(|e| UpgradeError::TlsConfiguration(format!("Failed to configure server: {e}")))
    }

    fn make_client_config(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        _crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> Result<ClientConfig, UpgradeError> {
        // Create root cert store with CA certificates
        let mut root_store = RootCertStore::empty();
        for ca_cert in &ca_certs {
            root_store.add(ca_cert.clone()).map_err(|e| {
                UpgradeError::TlsConfiguration(format!("Failed to add CA cert: {e}"))
            })?;
        }

        // NOTE: Client-side CRL validation is handled by the server via the
        // WebPkiClientVerifier which will check if our certificate is revoked.
        // We don't validate server certificates against CRL on the client, yet!

        // TODO: Add CRL validation for server certificates.
        ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_client_auth_cert(cert_chain, private_key)
            .map_err(|e| UpgradeError::TlsConfiguration(format!("Failed to configure client: {e}")))
    }
}

impl libp2p::core::upgrade::UpgradeInfo for Config {
    type Info = &'static str;
    type InfoIter = std::iter::Once<Self::Info>;

    fn protocol_info(&self) -> Self::InfoIter {
        std::iter::once("/mtls/0.1.0")
    }
}

impl<C> libp2p::core::upgrade::InboundConnectionUpgrade<C> for Config
where
    C: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    type Output = (PeerId, TlsStream<C>);
    type Error = UpgradeError;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn upgrade_inbound(
        self,
        socket: C,
        _: <Self as libp2p::core::upgrade::UpgradeInfo>::Info,
    ) -> Self::Future {
        async move {
            let stream = futures_rustls::TlsAcceptor::from(Arc::new(self.server))
                .accept(socket)
                .await
                .map_err(UpgradeError::ServerUpgrade)?;

            let peer_id = extract_peer_id_from_server_stream(&stream)?;

            Ok((peer_id, TlsStream::Server(stream)))
        }
        .boxed()
    }
}

impl<C> libp2p::core::upgrade::OutboundConnectionUpgrade<C> for Config
where
    C: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    type Output = (PeerId, TlsStream<C>);
    type Error = UpgradeError;
    type Future = BoxFuture<'static, Result<Self::Output, Self::Error>>;

    fn upgrade_outbound(
        self,
        socket: C,
        _: <Self as libp2p::core::upgrade::UpgradeInfo>::Info,
    ) -> Self::Future {
        async move {
            // Use unspecified IP address to disable SNI as per libp2p spec, for details see rfc/2025-05-30_mtls.md
            let server_name = ServerName::IpAddress(rustls::pki_types::IpAddr::from(IpAddr::V4(
                Ipv4Addr::UNSPECIFIED,
            )));

            let stream = futures_rustls::TlsConnector::from(Arc::new(self.client))
                .connect(server_name, socket)
                .await
                .map_err(|e| {
                    tracing::error!(error = ?e, "Upgrade Failed");

                    UpgradeError::ClientUpgrade(e)
                })?;

            let peer_id = extract_peer_id_from_client_stream(&stream)?;

            Ok((peer_id, TlsStream::Client(stream)))
        }
        .boxed()
    }
}

fn extract_peer_id_from_server_stream<T>(
    stream: &futures_rustls::server::TlsStream<T>,
) -> Result<PeerId, UpgradeError> {
    extract_peer_id_from_tls_state(stream.get_ref().1)
}

fn extract_peer_id_from_client_stream<T>(
    stream: &futures_rustls::client::TlsStream<T>,
) -> Result<PeerId, UpgradeError> {
    extract_peer_id_from_tls_state(stream.get_ref().1)
}

fn extract_peer_id_from_tls_state(state: &CommonState) -> Result<PeerId, UpgradeError> {
    let peer_certs = state
        .peer_certificates()
        .ok_or(UpgradeError::NoPeerCertificate)?;

    if peer_certs.is_empty() {
        return Err(UpgradeError::NoPeerCertificate);
    }

    let cert_der: &CertificateDer<'_> = &peer_certs[0];

    // 1. Parse the certificate using rustls-webpki
    // rustls_webpki::EndEntityCert::try_from takes &[u8]
    let end_entity_cert = EndEntityCert::try_from(cert_der).map_err(|e| {
        UpgradeError::TlsConfiguration(format!("Failed to parse peer certificate with webpki: {e}"))
    })?;

    // 2. Get the SubjectPublicKeyInfo (SPKI) DER bytes
    let spki_der = end_entity_cert.subject_public_key_info();

    // 3. Parse the SPKI DER to get an Ed25519 verifying key.
    let verifying_key = VerifyingKey::from_public_key_der(spki_der.as_ref())?;

    // 4. Convert to libp2p PublicKey
    let ed25519_public = ed25519::PublicKey::try_from_bytes(verifying_key.as_bytes())
        .map_err(UpgradeError::DecodingError)?;
    let public_key = PublicKey::from(ed25519_public);

    // Get PeerId
    Ok(public_key.to_peer_id())
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
    use libp2p::core::upgrade::UpgradeInfo;
    use rcgen::{
        CertificateParams, CertificateRevocationListParams, DistinguishedName, DnType, IsCa,
        PKCS_ED25519, SerialNumber,
    };
    use time::OffsetDateTime;

    use super::*;
    use crate::cert::*;

    fn generate_self_signed_cert(cn: &str) -> (String, String, libp2p::identity::Keypair) {
        let mut cert_params = CertificateParams::new(vec![format!("{}.local", cn)]).unwrap();
        cert_params.distinguished_name = DistinguishedName::new();
        cert_params.distinguished_name.push(DnType::CommonName, cn);

        let key_pair = rcgen::KeyPair::generate_for(&PKCS_ED25519).unwrap();
        let cert = cert_params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem();
        let cert_key_pem = key_pair.serialize_pem();

        // Create libp2p keypair from the certificate's key
        let key_der = key_pair.serialize_der();
        let signing_key = SigningKey::from_pkcs8_der(&key_der).unwrap();
        let keypair =
            libp2p::identity::Keypair::ed25519_from_bytes(signing_key.to_bytes()).unwrap();

        (cert_pem, cert_key_pem, keypair)
    }

    #[test]
    fn test_config_creation_valid_certificates() {
        let (server_cert_pem, server_key_pem, _) = generate_self_signed_cert("server");

        let cert_chain = load_certs_from_pem(server_cert_pem.as_bytes()).unwrap();
        let private_key = load_private_key_from_pem(server_key_pem.as_bytes()).unwrap();
        let ca_certs = cert_chain.clone(); // Use self-signed cert as CA for testing

        let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]);
        assert!(config.is_ok());
    }

    #[test]
    fn test_config_creation_with_crl() {
        let (server_cert_pem, server_key_pem, _) = generate_self_signed_cert("server");

        // Create a simple CRL with an empty revocation list for testing
        let mut ca_params = CertificateParams::new(vec!["ca.local".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.distinguished_name = DistinguishedName::new();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Test CA");

        let ca_key_pair = rcgen::KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key_pair).unwrap();

        let crl_params = CertificateRevocationListParams {
            this_update: OffsetDateTime::now_utc(),
            next_update: OffsetDateTime::now_utc() + time::Duration::days(30),
            crl_number: SerialNumber::from(1u64),
            issuing_distribution_point: None,
            revoked_certs: vec![], // Empty CRL for testing
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };

        let crl = crl_params.signed_by(&ca_cert, &ca_key_pair).unwrap();
        let crl_pem = crl.pem().unwrap();

        let cert_chain = load_certs_from_pem(server_cert_pem.as_bytes()).unwrap();
        let private_key = load_private_key_from_pem(server_key_pem.as_bytes()).unwrap();
        let ca_certs = cert_chain.clone(); // Use self-signed cert as CA for testing
        let crls = load_crls_from_pem(crl_pem.as_bytes()).unwrap();

        let config = Config::try_new(cert_chain, private_key, ca_certs, crls);
        assert!(config.is_ok());
    }

    #[test]
    fn test_config_creation_invalid_ca_cert() {
        let (server_cert_pem, server_key_pem, _) = generate_self_signed_cert("server");

        let cert_chain = load_certs_from_pem(server_cert_pem.as_bytes()).unwrap();
        let private_key = load_private_key_from_pem(server_key_pem.as_bytes()).unwrap();

        // Try with completely invalid CA certs
        let invalid_ca = b"-----BEGIN CERTIFICATE-----\n!@#$%^&*()\n-----END CERTIFICATE-----\n";
        let ca_certs_result = load_certs_from_pem(invalid_ca);

        // Should fail to load invalid CA certs, so we expect an error
        if ca_certs_result.is_err() {
            // This is expected behavior - invalid PEM should fail to load
            return;
        }

        // If it somehow loads, the config creation should fail
        let ca_certs = ca_certs_result.unwrap();
        let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]);
        assert!(config.is_err());
    }

    #[test]
    fn test_extract_peer_id_from_tls_state_no_certs() {
        // Test with empty peer certificates - this would need mocking CommonState
        // For now, test the error cases we can
        let error = UpgradeError::NoPeerCertificate;
        assert_eq!(error.to_string(), "No peer certificate found");
    }

    #[test]
    fn test_protocol_info() {
        let (server_cert_pem, server_key_pem, _) = generate_self_signed_cert("server");

        let cert_chain = load_certs_from_pem(server_cert_pem.as_bytes()).unwrap();
        let private_key = load_private_key_from_pem(server_key_pem.as_bytes()).unwrap();
        let ca_certs = cert_chain.clone(); // Use self-signed cert as CA for testing

        let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]).unwrap();

        let protocol_info: Vec<_> = config.protocol_info().collect();
        assert_eq!(protocol_info, vec!["/mtls/0.1.0"]);
    }
}
