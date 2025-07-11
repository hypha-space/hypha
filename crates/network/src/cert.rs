//! Handle certificates and peer identities.

use std::io;

use ed25519_dalek::{SigningKey, pkcs8::DecodePrivateKey};
use rustls::pki_types::{CertificateDer, CertificateRevocationListDer, PrivateKeyDer};
use thiserror::Error;

/// Errors that can occur when parsing certificates.
#[derive(Error, Debug)]
pub enum ParseError {
    /// Invalid certificate format
    #[error("Invalid certificate format")]
    InvalidFormat,
    /// Unsupported public key algorithm
    #[error("Unsupported public key algorithm")]
    UnsupportedPublicKey,
    /// Public key extraction failed
    #[error("Failed to extract public key")]
    PublicKeyExtraction,
    /// Key-pair creation failed
    #[error("Failed to create keypair: {0}")]
    KeypairCreation(String),
    /// IO error
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
}

/// Create a libp2p identity from a private key
pub fn identity_from_private_key(
    private_key: &rustls::pki_types::PrivateKeyDer<'static>,
) -> Result<libp2p::identity::Keypair, ParseError> {
    match private_key {
        PrivateKeyDer::Pkcs8(key) => {
            let key = SigningKey::from_pkcs8_der(key.secret_pkcs8_der()).map_err(|_| {
                ParseError::KeypairCreation(
                    "Invalid PKCS8 format: too short for Ed25519 private key".to_string(),
                )
            })?;

            libp2p::identity::Keypair::ed25519_from_bytes(key.to_bytes()).map_err(|_| {
                ParseError::KeypairCreation(
                    "Invalid PKCS8 format: too short for Ed25519 private key".to_string(),
                )
            })
        }
        _ => Err(ParseError::InvalidFormat),
    }
}

/// Load certificates from PEM data
pub fn load_certs_from_pem(pem_data: &[u8]) -> Result<Vec<CertificateDer<'static>>, io::Error> {
    rustls_pemfile::certs(&mut io::Cursor::new(pem_data)).collect::<Result<Vec<_>, _>>()
}

/// Load private key from PEM data
pub fn load_private_key_from_pem(
    pem_data: &[u8],
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, io::Error> {
    let mut cursor = io::Cursor::new(pem_data);

    // TODO: Try different key formats once we support more key types and formats
    if let Some(Ok(key)) = rustls_pemfile::pkcs8_private_keys(&mut cursor).next() {
        return Ok(rustls::pki_types::PrivateKeyDer::Pkcs8(key));
    }

    Err(io::Error::new(
        io::ErrorKind::InvalidData,
        "No valid private key found in PEM data",
    ))
}

/// Load CRLs from PEM data
pub fn load_crls_from_pem(
    pem_data: &[u8],
) -> Result<Vec<CertificateRevocationListDer<'static>>, io::Error> {
    rustls_pemfile::crls(&mut io::Cursor::new(pem_data))
        .map(|crl| crl.map(|c| c.to_owned()))
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::SigningKey;
    use rcgen::{
        CertificateParams, CertificateRevocationListParams, DistinguishedName, DnType, IsCa,
        PKCS_ED25519, RevokedCertParams, SerialNumber,
    };
    use time::OffsetDateTime;

    use super::*;

    fn generate_ed25519_cert() -> (String, String, libp2p::identity::Keypair) {
        let mut params = CertificateParams::new(vec!["test.local".to_string()]).unwrap();
        params.distinguished_name = DistinguishedName::new();
        params.distinguished_name.push(DnType::CommonName, "Test");

        let key_pair = rcgen::KeyPair::generate_for(&PKCS_ED25519).unwrap();
        let cert = params.self_signed(&key_pair).unwrap();
        let cert_pem = cert.pem();
        let key_pem = key_pair.serialize_pem();

        // Extract the key to create libp2p keypair
        let key_der = key_pair.serialize_der();
        let signing_key = SigningKey::from_pkcs8_der(&key_der).unwrap();
        let keypair =
            libp2p::identity::Keypair::ed25519_from_bytes(signing_key.to_bytes()).unwrap();

        (cert_pem, key_pem, keypair)
    }

    #[test]
    fn test_load_certs_from_valid_pem() {
        let (cert_pem, _, _) = generate_ed25519_cert();

        let certs = load_certs_from_pem(cert_pem.as_bytes()).unwrap();
        assert_eq!(certs.len(), 1);
    }

    #[test]
    fn test_load_certs_from_invalid_pem() {
        let invalid_pem = "-----BEGIN CERTIFICATE-----\n!@#$%^&*()\n-----END CERTIFICATE-----\n";
        let result = load_certs_from_pem(invalid_pem.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_certs_from_empty_pem() {
        let certs = load_certs_from_pem(b"").unwrap();
        assert_eq!(certs.len(), 0);
    }

    #[test]
    fn test_load_private_key_ed25519() {
        let (_, key_pem, _) = generate_ed25519_cert();

        let key = load_private_key_from_pem(key_pem.as_bytes()).unwrap();
        match key {
            PrivateKeyDer::Pkcs8(_) => {} // Expected
            _ => panic!("Expected PKCS8 key format for Ed25519"),
        }
    }

    #[test]
    fn test_load_private_key_invalid() {
        let invalid_pem = "-----BEGIN PRIVATE KEY-----\n!@#$%^&*()\n-----END PRIVATE KEY-----\n";
        let result = load_private_key_from_pem(invalid_pem.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_load_private_key_no_key_found() {
        let cert_pem = "-----BEGIN CERTIFICATE-----\nMIIBIjANBgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEA7\n-----END CERTIFICATE-----\n";
        let result = load_private_key_from_pem(cert_pem.as_bytes());
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No valid private key found")
        );
    }

    #[test]
    fn test_identity_from_ed25519_private_key() {
        let (_, key_pem, expected_keypair) = generate_ed25519_cert();
        let key = load_private_key_from_pem(key_pem.as_bytes()).unwrap();

        let identity = identity_from_private_key(&key).unwrap();

        // Verify the identity matches the expected keypair
        assert_eq!(identity.public(), expected_keypair.public());
    }

    #[test]
    fn test_identity_from_invalid_ed25519_key() {
        // Create an invalid PKCS8 key (too short)
        let invalid_pkcs8 = rustls::pki_types::PrivatePkcs8KeyDer::from(vec![1, 2, 3]);
        let key = PrivateKeyDer::Pkcs8(invalid_pkcs8);

        let result = identity_from_private_key(&key);
        assert!(matches!(result, Err(ParseError::KeypairCreation(_))));
    }

    #[test]
    fn test_load_crls_from_pem() {
        // Generate CA cert with built-in key generation
        let mut ca_params = CertificateParams::new(vec!["ca.local".to_string()]).unwrap();
        ca_params.is_ca = IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        ca_params.distinguished_name = DistinguishedName::new();
        ca_params
            .distinguished_name
            .push(DnType::CommonName, "Test CA");

        let ca_key_pair = rcgen::KeyPair::generate().unwrap();
        let ca_cert = ca_params.self_signed(&ca_key_pair).unwrap();

        // Create CRL
        let revoked_cert = RevokedCertParams {
            serial_number: SerialNumber::from(123u64),
            revocation_time: OffsetDateTime::now_utc(),
            reason_code: Some(rcgen::RevocationReason::KeyCompromise),
            invalidity_date: None,
        };

        let crl_params = CertificateRevocationListParams {
            this_update: OffsetDateTime::now_utc(),
            next_update: OffsetDateTime::now_utc() + time::Duration::days(30),
            crl_number: SerialNumber::from(1u64),
            issuing_distribution_point: None,
            revoked_certs: vec![revoked_cert],
            key_identifier_method: rcgen::KeyIdMethod::Sha256,
        };

        let crl = crl_params.signed_by(&ca_cert, &ca_key_pair).unwrap();
        let crl_pem = crl.pem().unwrap();

        let crls = load_crls_from_pem(crl_pem.as_bytes()).unwrap();
        assert_eq!(crls.len(), 1);
    }

    #[test]
    fn test_load_empty_crls() {
        let crls = load_crls_from_pem(b"").unwrap();
        assert_eq!(crls.len(), 0);
    }

    #[test]
    fn test_load_crls_invalid_pem() {
        let invalid_crl_pem = "-----BEGIN X509 CRL-----\n!@#$%^&*()\n-----END X509 CRL-----\n";
        let result = load_crls_from_pem(invalid_crl_pem.as_bytes());
        assert!(result.is_err());
    }

    #[test]
    fn test_multiple_certs_in_pem() {
        let (cert1_pem, _, _) = generate_ed25519_cert();
        let (cert2_pem, _, _) = generate_ed25519_cert();

        let combined_pem = format!("{cert1_pem}\n{cert2_pem}");
        let certs = load_certs_from_pem(combined_pem.as_bytes()).unwrap();
        assert_eq!(certs.len(), 2);
    }
}
