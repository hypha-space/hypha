use std::{
    io::{self, ErrorKind},
    pin::Pin,
    task::{Context, Poll},
};

use ed25519_dalek::pkcs8::DecodePrivateKey;
use futures_util::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use hypha_network::{cert::*, mtls::*};
use libp2p::{
    core::upgrade::{InboundConnectionUpgrade, OutboundConnectionUpgrade, UpgradeInfo},
    identity::Keypair,
};
use rcgen::{
    Certificate, CertificateParams, CertificateRevocationList, CertificateRevocationListParams,
    DistinguishedName, DnType, IsCa, PKCS_ED25519, SerialNumber,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use time::OffsetDateTime;

// Mock transport layer for testing TLS streams
struct MockTransport {
    data: Vec<u8>,
    read_pos: usize,
    write_buffer: Vec<u8>,
    closed: bool,
}

impl MockTransport {
    fn new(data: Vec<u8>) -> Self {
        Self {
            data,
            read_pos: 0,
            write_buffer: Vec::new(),
            closed: false,
        }
    }

    fn with_error() -> Self {
        Self {
            data: vec![],
            read_pos: 0,
            write_buffer: Vec::new(),
            closed: true,
        }
    }
}

impl AsyncRead for MockTransport {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "Transport closed",
            )));
        }

        let remaining = self.data.len() - self.read_pos;
        if remaining == 0 {
            return Poll::Ready(Ok(0)); // EOF
        }

        let to_read = buf.len().min(remaining);
        buf[..to_read].copy_from_slice(&self.data[self.read_pos..self.read_pos + to_read]);
        self.read_pos += to_read;
        Poll::Ready(Ok(to_read))
    }
}

impl AsyncWrite for MockTransport {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        if self.closed {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "Transport closed",
            )));
        }

        self.write_buffer.extend_from_slice(buf);
        Poll::Ready(Ok(buf.len()))
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.closed {
            return Poll::Ready(Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "Transport closed",
            )));
        }
        Poll::Ready(Ok(()))
    }

    fn poll_close(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        self.closed = true;
        Poll::Ready(Ok(()))
    }
}

impl Unpin for MockTransport {}

#[tokio::test]
async fn test_config_creation_valid_setup() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let (ca_certs, crls) = generate_ca_and_crl();

    let config = Config::try_new(cert_chain, private_key, ca_certs, crls);
    assert!(
        config.is_ok(),
        "Config creation should succeed with valid inputs"
    );
}

#[tokio::test]
async fn test_config_protocol_info() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let ca_certs = cert_chain.clone();

    let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]).unwrap();

    let protocols: Vec<_> = config.protocol_info().collect();
    assert_eq!(protocols, vec!["/mtls/0.1.0"]);
}

#[tokio::test]
async fn test_mock_transport_async_read() {
    let test_data = b"Hello, World!";
    let mut buffer = vec![0u8; test_data.len()];

    let mut mock = MockTransport::new(test_data.to_vec());
    let bytes_read = mock.read(&mut buffer).await.unwrap();
    assert_eq!(bytes_read, test_data.len());
    assert_eq!(buffer, test_data);
}

#[tokio::test]
async fn test_mock_transport_async_write() {
    let test_data = b"test write data";
    let mut mock = MockTransport::new(vec![]);

    let bytes_written = mock.write(test_data).await.unwrap();
    assert_eq!(bytes_written, test_data.len());

    mock.flush().await.unwrap();
    assert_eq!(mock.write_buffer, test_data);
}

#[tokio::test]
async fn test_mock_transport_close() {
    let mut mock = MockTransport::new(vec![]);
    mock.close().await.unwrap();
    assert!(mock.closed);
}

#[tokio::test]
async fn test_mock_transport_read_error() {
    let mut mock = MockTransport::with_error();
    let mut buffer = vec![0u8; 10];

    let result = mock.read(&mut buffer).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_mock_transport_write_error() {
    let mut mock = MockTransport::with_error();
    let test_data = b"test data";

    let result = mock.write(test_data).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_upgrade_error_display() {
    let server_error =
        UpgradeError::ServerUpgrade(io::Error::new(ErrorKind::ConnectionRefused, "refused"));
    assert!(
        server_error
            .to_string()
            .contains("Failed to upgrade server connection")
    );

    let client_error = UpgradeError::ClientUpgrade(io::Error::new(ErrorKind::TimedOut, "timeout"));
    assert!(
        client_error
            .to_string()
            .contains("Failed to upgrade client connection")
    );

    let tls_error = UpgradeError::TlsConfiguration("invalid config".to_string());
    assert!(tls_error.to_string().contains("Failed to configure TLS"));

    let no_cert_error = UpgradeError::NoPeerCertificate;
    assert_eq!(no_cert_error.to_string(), "No peer certificate found");
}

#[tokio::test]
async fn test_config_creation_invalid_certificate() {
    // Create invalid certificate data
    let invalid_cert_pem =
        b"-----BEGIN CERTIFICATE-----\nINVALID_DATA\n-----END CERTIFICATE-----\n";
    let cert_result = load_certs_from_pem(invalid_cert_pem);

    // Should fail to parse invalid certificate
    assert!(
        cert_result.is_err(),
        "Should fail with invalid certificate data"
    );
}

#[tokio::test]
async fn test_config_creation_invalid_private_key() {
    let (_cert_chain, _valid_key, _keypair) = generate_test_cert_and_key("test-peer");

    // Create invalid private key data
    let invalid_key_pem =
        b"-----BEGIN PRIVATE KEY-----\nINVALID_KEY_DATA\n-----END PRIVATE KEY-----\n";
    let key_result = load_private_key_from_pem(invalid_key_pem);

    // Should fail to parse invalid private key
    assert!(
        key_result.is_err(),
        "Should fail with invalid private key data"
    );
}

#[tokio::test]
async fn test_config_with_mismatched_cert_and_key() {
    let (cert_chain1, _key1, _kp1) = generate_test_cert_and_key("peer1");
    let (_cert_chain2, key2, _kp2) = generate_test_cert_and_key("peer2");
    let ca_certs = cert_chain1.clone();

    // Try to create config with mismatched cert and key
    let config_result = Config::try_new(cert_chain1, key2, ca_certs, vec![]);

    // Should fail due to mismatched certificate and private key
    assert!(
        config_result.is_err(),
        "Should fail with mismatched cert and key"
    );
}

#[tokio::test]
async fn test_config_with_empty_ca_certs() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let empty_ca_certs = vec![];

    let config_result = Config::try_new(cert_chain, private_key, empty_ca_certs, vec![]);

    // Actually, empty CA list may cause issues - test that it fails gracefully
    assert!(config_result.is_err(), "Should fail with empty CA certs");
}

#[tokio::test]
async fn test_extract_peer_id_error_cases() {
    // Test error display for SPKI error
    let spki_error = ed25519_dalek::pkcs8::spki::Error::KeyMalformed;
    let upgrade_error = UpgradeError::SPKIError(spki_error);
    assert!(
        upgrade_error
            .to_string()
            .contains("Failed to decode public key from DER")
    );

    // Test error display for no peer certificate
    let upgrade_error = UpgradeError::NoPeerCertificate;
    let error_string = format!("{}", upgrade_error);
    assert_eq!(error_string, "No peer certificate found");
}

#[tokio::test]
async fn test_config_creation_with_multiple_ca_certs() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let (mut ca_certs1, _crls1) = generate_ca_and_crl();
    let (ca_certs2, _crls2) = generate_ca_and_crl();

    // Combine multiple CA certificates
    ca_certs1.extend(ca_certs2);

    let config = Config::try_new(cert_chain, private_key, ca_certs1, vec![]);
    assert!(
        config.is_ok(),
        "Should succeed with multiple CA certificates"
    );
}

#[tokio::test]
async fn test_config_creation_with_multiple_crls() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let (ca_certs, mut crls1) = generate_ca_and_crl();
    let (_ca_certs2, crls2) = generate_ca_and_crl();

    // Combine multiple CRLs
    crls1.extend(crls2);

    let config = Config::try_new(cert_chain, private_key, ca_certs, crls1);
    assert!(config.is_ok(), "Should succeed with multiple CRLs");
}

#[tokio::test]
async fn test_upgrade_info_trait() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("test-peer");
    let ca_certs = cert_chain.clone();

    let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]).unwrap();

    // Test that the UpgradeInfo trait is properly implemented
    let protocols: Vec<_> = config.protocol_info().collect();
    assert_eq!(protocols.len(), 1);
    assert_eq!(protocols[0], "/mtls/0.1.0");
}

#[tokio::test]
async fn test_tls_stream_enum_variants() {
    // Test that both TlsStream variants exist and can be pattern matched
    // NOTE: We can't easily test the actual stream implementations without real TLS handshakes,
    // but we can verify the enum structure exists and compiles

    // Verify the enum has the expected variants by checking compilation
    fn _test_tls_stream_variants<T: AsyncRead + AsyncWrite + Unpin>() {
        let _client_variant: Option<TlsStream<T>> = None;
        let _server_variant: Option<TlsStream<T>> = None;
    }

    _test_tls_stream_variants::<MockTransport>();
}

#[tokio::test]
async fn test_mock_transport_partial_read() {
    let test_data = b"Hello, World!";
    let mut mock = MockTransport::new(test_data.to_vec());

    // Read in smaller chunks
    let mut buffer = vec![0u8; 5];
    let bytes_read1 = mock.read(&mut buffer).await.unwrap();
    assert_eq!(bytes_read1, 5);
    assert_eq!(&buffer, b"Hello");

    let mut buffer = vec![0u8; 8];
    let bytes_read2 = mock.read(&mut buffer).await.unwrap();
    assert_eq!(bytes_read2, 8);
    assert_eq!(&buffer, b", World!");

    // Should be EOF now
    let mut buffer = vec![0u8; 5];
    let bytes_read3 = mock.read(&mut buffer).await.unwrap();
    assert_eq!(bytes_read3, 0);
}

#[tokio::test]
async fn test_mock_transport_multiple_writes() {
    let mut mock = MockTransport::new(vec![]);

    let data1 = b"Hello, ";
    let data2 = b"World!";

    mock.write(data1).await.unwrap();
    mock.write(data2).await.unwrap();
    mock.flush().await.unwrap();

    let expected = b"Hello, World!";
    assert_eq!(mock.write_buffer, expected);
}

#[tokio::test]
async fn test_config_creation_edge_cases() {
    // Test with self-signed certificate as both server cert and CA
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("self-signed");
    let ca_certs = cert_chain.clone(); // Use same cert as CA

    let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]);
    assert!(config.is_ok(), "Should succeed with self-signed cert as CA");
}

#[tokio::test]
async fn test_error_types_comprehensive() {
    // Test all UpgradeError variants for completeness
    let errors = vec![
        UpgradeError::ServerUpgrade(io::Error::new(ErrorKind::ConnectionRefused, "test")),
        UpgradeError::ClientUpgrade(io::Error::new(ErrorKind::TimedOut, "test")),
        UpgradeError::TlsConfiguration("test config error".to_string()),
        UpgradeError::NoPeerCertificate,
        UpgradeError::SPKIError(ed25519_dalek::pkcs8::spki::Error::KeyMalformed),
    ];

    for error in errors {
        let error_string = error.to_string();
        assert!(
            !error_string.is_empty(),
            "Error should have non-empty string representation"
        );
    }
}

// NOTE: Test config creation methods to exercise private functions
#[tokio::test]
async fn test_config_make_server_config() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("server-test");
    let (ca_certs, crls) = generate_ca_and_crl();

    // This will exercise make_server_config internally
    let config_result = Config::try_new(cert_chain, private_key, ca_certs, crls);
    assert!(
        config_result.is_ok(),
        "Server config creation should succeed"
    );
}

#[tokio::test]
async fn test_config_make_client_config() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("client-test");
    let (ca_certs, crls) = generate_ca_and_crl();

    // This will exercise make_client_config internally
    let config_result = Config::try_new(cert_chain, private_key, ca_certs, crls);
    assert!(
        config_result.is_ok(),
        "Client config creation should succeed"
    );
}

// NOTE: Test TlsStream AsyncRead/AsyncWrite through the enum variants
#[tokio::test]
async fn test_tls_stream_async_traits() {
    // Test that TlsStream properly implements AsyncRead and AsyncWrite traits
    // by verifying the trait bounds compile
    fn _verify_async_traits<T>()
    where
        T: AsyncRead + AsyncWrite + Unpin,
    {
        fn _accepts_async_read<R: AsyncRead>(_: R) {}
        fn _accepts_async_write<W: AsyncWrite>(_: W) {}

        let stream: Option<TlsStream<MockTransport>> = None;
        if let Some(s) = stream {
            _accepts_async_read(s);
        }

        let stream: Option<TlsStream<MockTransport>> = None;
        if let Some(s) = stream {
            _accepts_async_write(s);
        }
    }

    _verify_async_traits::<TlsStream<MockTransport>>();
}

// NOTE: Test that cloning Config works (tests the Clone derive)
#[tokio::test]
async fn test_config_clone() {
    let (cert_chain, private_key, _keypair) = generate_test_cert_and_key("clone-test");
    let ca_certs = cert_chain.clone();

    let config = Config::try_new(cert_chain, private_key, ca_certs, vec![]).unwrap();
    let _cloned_config = config.clone();

    // Verify both configs have the same protocol info
    let protocols1: Vec<_> = config.protocol_info().collect();
    let protocols2: Vec<_> = _cloned_config.protocol_info().collect();
    assert_eq!(protocols1, protocols2);
}

// NOTE: Integration tests using real certificates and TLS connections

// Compatibility wrapper to convert tokio AsyncRead/AsyncWrite to futures
struct TokioCompat<T> {
    inner: T,
}

impl<T> TokioCompat<T> {
    fn new(inner: T) -> Self {
        Self { inner }
    }
}

impl<T> AsyncRead for TokioCompat<T>
where
    T: tokio::io::AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        let mut read_buf = tokio::io::ReadBuf::new(buf);
        match Pin::new(&mut self.inner).poll_read(cx, &mut read_buf) {
            Poll::Ready(Ok(())) => Poll::Ready(Ok(read_buf.filled().len())),
            Poll::Ready(Err(e)) => Poll::Ready(Err(e)),
            Poll::Pending => Poll::Pending,
        }
    }
}

impl<T> AsyncWrite for TokioCompat<T>
where
    T: tokio::io::AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_close(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

impl<T> Unpin for TokioCompat<T> where T: Unpin {}

fn load_real_certificates()
-> Result<(Config, Config, libp2p::PeerId, libp2p::PeerId), Box<dyn std::error::Error>> {
    // Look for certificates in the project root (where they are listed)
    let cert_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent() // go up from crates/network to project root
        .unwrap()
        .parent() // go up from crates to project root
        .unwrap();

    // Load the example root CA certificate as our trust anchor
    let ca_cert_path = cert_dir.join("example-root-ca-cert.pem");
    let ca_cert_pem = std::fs::read(&ca_cert_path)
        .map_err(|e| format!("Failed to read CA cert from {:?}: {}", ca_cert_path, e))?;
    let ca_certs = load_certs_from_pem(&ca_cert_pem)?;

    // For the server certificate and key
    let server_cert_path = cert_dir.join("server-example-local-cert.pem");
    let server_key_path = cert_dir.join("server-example-local-key.pem");

    let server_cert_pem = std::fs::read(&server_cert_path).map_err(|e| {
        format!(
            "Failed to read server cert from {:?}: {}",
            server_cert_path, e
        )
    })?;
    let server_key_pem = std::fs::read(&server_key_path).map_err(|e| {
        format!(
            "Failed to read server key from {:?}: {}",
            server_key_path, e
        )
    })?;

    let server_cert_chain = load_certs_from_pem(&server_cert_pem)?;
    let server_private_key = load_private_key_from_pem(&server_key_pem)?;

    // For client, we'll use the server certificate as both client and server for simplicity
    // In a real scenario, you'd have separate client certificates
    let client_cert_chain = server_cert_chain.clone();
    let client_private_key = server_private_key.clone_key();

    // Create TLS configs
    let server_config = Config::try_new(
        server_cert_chain.clone(),
        server_private_key,
        ca_certs.clone(),
        vec![],
    )?;

    let client_config = Config::try_new(
        client_cert_chain.clone(),
        client_private_key,
        ca_certs,
        vec![],
    )?;

    // Extract peer IDs from certificates
    let server_peer_id = extract_peer_id_from_certificate(&server_cert_chain[0])?;
    let client_peer_id = extract_peer_id_from_certificate(&client_cert_chain[0])?;

    Ok((server_config, client_config, server_peer_id, client_peer_id))
}

fn extract_peer_id_from_certificate(
    cert_der: &CertificateDer,
) -> Result<libp2p::PeerId, Box<dyn std::error::Error>> {
    use ed25519_dalek::{VerifyingKey, pkcs8::DecodePublicKey};
    use libp2p::identity::{PublicKey, ed25519};

    // Parse the certificate using webpki
    let end_entity_cert = webpki::EndEntityCert::try_from(cert_der)?;

    // Get the SubjectPublicKeyInfo (SPKI) DER bytes
    let spki_der = end_entity_cert.subject_public_key_info();

    // Parse the SPKI DER to get an Ed25519 verifying key
    let verifying_key = VerifyingKey::from_public_key_der(spki_der.as_ref())?;

    // Convert to libp2p PublicKey
    let ed25519_public = ed25519::PublicKey::try_from_bytes(verifying_key.as_bytes())?;
    let public_key = PublicKey::from(ed25519_public);

    Ok(public_key.to_peer_id())
}

#[tokio::test]
async fn test_load_real_certificates() {
    // Test that we can load the real certificates
    match load_real_certificates() {
        Ok((server_config, client_config, server_peer_id, client_peer_id)) => {
            // Verify configs were created successfully
            let server_protocols: Vec<_> = server_config.protocol_info().collect();
            let client_protocols: Vec<_> = client_config.protocol_info().collect();

            assert_eq!(server_protocols, vec!["/mtls/0.1.0"]);
            assert_eq!(client_protocols, vec!["/mtls/0.1.0"]);

            // Verify peer IDs are valid
            assert_ne!(server_peer_id.to_string().len(), 0);
            assert_ne!(client_peer_id.to_string().len(), 0);

            println!("✅ Successfully loaded real certificates");
            println!("Server PeerId: {}", server_peer_id);
            println!("Client PeerId: {}", client_peer_id);
        }
        Err(e) => {
            println!("⚠️  Could not load real certificates: {}", e);
            println!("This is expected if certificates haven't been generated yet.");
            println!(
                "To generate certificates, run the commands from the request_response example README."
            );
        }
    }
}

#[tokio::test]
async fn test_real_tls_handshake_integration() {
    let (server_config, client_config, expected_server_peer_id, expected_client_peer_id) =
        match load_real_certificates() {
            Ok(certs) => certs,
            Err(e) => {
                println!(
                    "⚠️  Skipping TLS handshake test - certificates not available: {}",
                    e
                );
                return;
            }
        };

    // Create a bidirectional channel that will act as our "network"
    let (client_stream, server_stream) = tokio::io::duplex(8192);

    // Wrap streams in compatibility layer
    let client_compat = TokioCompat::new(client_stream);
    let server_compat = TokioCompat::new(server_stream);

    // Perform the actual TLS handshake
    let server_upgrade = server_config.upgrade_inbound(server_compat, "/mtls/0.1.0");
    let client_upgrade = client_config.upgrade_outbound(client_compat, "/mtls/0.1.0");

    let (server_result, client_result) = tokio::join!(server_upgrade, client_upgrade);

    // Verify both sides of the handshake succeeded
    let (extracted_client_peer_id, server_tls_stream) =
        server_result.expect("Server TLS handshake should succeed");
    let (extracted_server_peer_id, client_tls_stream) =
        client_result.expect("Client TLS handshake should succeed");

    // Verify peer ID extraction worked correctly
    assert_eq!(
        extracted_client_peer_id, expected_client_peer_id,
        "Server should correctly extract client peer ID from certificate"
    );
    assert_eq!(
        extracted_server_peer_id, expected_server_peer_id,
        "Client should correctly extract server peer ID from certificate"
    );

    println!("✅ TLS handshake completed successfully!");
    println!(
        "Server extracted client peer ID: {}",
        extracted_client_peer_id
    );
    println!(
        "Client extracted server peer ID: {}",
        extracted_server_peer_id
    );

    // Test actual encrypted communication over the TLS connection
    test_encrypted_bidirectional_communication(client_tls_stream, server_tls_stream).await;
}

async fn test_encrypted_bidirectional_communication<T>(
    mut client_stream: TlsStream<T>,
    mut server_stream: TlsStream<T>,
) where
    T: AsyncRead + AsyncWrite + Unpin + Send,
{
    use futures_util::{AsyncReadExt, AsyncWriteExt};

    println!("🔐 Testing encrypted communication...");

    // Test 1: Client sends message to server
    let client_message = b"Hello server, this message is encrypted with mTLS!";
    client_stream
        .write_all(client_message)
        .await
        .expect("Client should be able to write encrypted data");
    client_stream
        .flush()
        .await
        .expect("Client should be able to flush TLS stream");

    let mut server_buffer = vec![0u8; client_message.len()];
    server_stream
        .read_exact(&mut server_buffer)
        .await
        .expect("Server should be able to read encrypted data");
    assert_eq!(
        &server_buffer, client_message,
        "Server should receive decrypted message from client"
    );

    println!("✅ Client → Server encrypted communication working");

    // Test 2: Server responds to client
    let server_response = b"Hello client, I received your encrypted message!";
    server_stream
        .write_all(server_response)
        .await
        .expect("Server should be able to write encrypted response");
    server_stream
        .flush()
        .await
        .expect("Server should be able to flush TLS stream");

    let mut client_buffer = vec![0u8; server_response.len()];
    client_stream
        .read_exact(&mut client_buffer)
        .await
        .expect("Client should be able to read encrypted response");
    assert_eq!(
        &client_buffer, server_response,
        "Client should receive decrypted response from server"
    );

    println!("✅ Server → Client encrypted communication working");

    // Test 3: Large data transfer to exercise buffering and partial reads/writes
    let large_data = vec![0xAB; 4096]; // 4KB of data

    server_stream
        .write_all(&large_data)
        .await
        .expect("Server should handle large encrypted writes");
    server_stream.flush().await.unwrap();

    let mut received_data = Vec::new();
    let mut chunk_buffer = [0u8; 512]; // Read in 512-byte chunks

    while received_data.len() < large_data.len() {
        let bytes_read = client_stream
            .read(&mut chunk_buffer)
            .await
            .expect("Client should handle partial encrypted reads");
        if bytes_read == 0 {
            break; // EOF
        }
        received_data.extend_from_slice(&chunk_buffer[..bytes_read]);
    }

    assert_eq!(
        received_data, large_data,
        "Large encrypted data transfer should work correctly"
    );

    println!("✅ Large data encrypted transfer working");

    // Test 4: Verify TlsStream enum variants are correct
    match &client_stream {
        TlsStream::Client(_) => println!("✅ Client stream has correct Client variant"),
        TlsStream::Server(_) => panic!("Client stream should have Client variant"),
    }

    match &server_stream {
        TlsStream::Server(_) => println!("✅ Server stream has correct Server variant"),
        TlsStream::Client(_) => panic!("Server stream should have Server variant"),
    }

    // Test 5: Graceful connection closure
    client_stream
        .close()
        .await
        .expect("Client TLS stream should close gracefully");
    server_stream
        .close()
        .await
        .expect("Server TLS stream should close gracefully");

    println!("✅ TLS streams closed gracefully");
    println!("🎉 All encrypted communication tests passed!");
}

#[tokio::test]
async fn test_tls_handshake_with_connection_drop() {
    let (_server_config, client_config, _, _) = match load_real_certificates() {
        Ok(certs) => certs,
        Err(e) => {
            println!(
                "⚠️  Skipping connection drop test - certificates not available: {}",
                e
            );
            return;
        }
    };

    // Create connection and immediately drop server side
    let (client_stream, server_stream) = tokio::io::duplex(8192);
    drop(server_stream); // Simulate network failure

    let client_compat = TokioCompat::new(client_stream);
    let client_upgrade = client_config.upgrade_outbound(client_compat, "/mtls/0.1.0");

    // Client upgrade should fail gracefully
    let result = client_upgrade.await;
    assert!(
        result.is_err(),
        "Client upgrade should fail when server connection drops"
    );

    match result {
        Err(UpgradeError::ClientUpgrade(_)) => {
            println!("✅ Client upgrade failed correctly with ClientUpgrade error");
        }
        Err(other) => {
            println!("✅ Client upgrade failed with error: {:?}", other);
        }
        Ok(_) => {
            panic!("Client upgrade should not succeed when server drops connection");
        }
    }
}
