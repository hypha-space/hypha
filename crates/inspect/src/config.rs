use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::{ConfigWithMetadata, NetworkConfig, TLSConfig, ValidatableConfig};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Inspector configuration for network diagnostics and introspection.
pub struct Config {
    /// Path to the TLS certificate PEM file.
    ///
    /// Must be a valid X.509 certificate in PEM format that establishes the inspector's
    /// identity when connecting to Hypha gateways.
    ///
    /// SECURITY: Use certificates from a recognized CA or internal PKI for deployments.
    cert_pem: PathBuf,

    /// Path to the private key PEM file.
    ///
    /// Must correspond to cert_pem. This is the inspector's cryptographic identity.
    ///
    /// SECURITY:
    ///   * Restrict file permissions (chmod 600 recommended)
    ///   * Never commit to version control
    ///   * Store securely using secrets management systems in production
    ///   * Keep encrypted backups for recovery
    key_pem: PathBuf,

    /// Path to the trust chain PEM file (CA bundle).
    ///
    /// Contains root and intermediate certificates trusted by the inspector. Peers presenting
    /// certificates signed by these CAs will be accepted for network connections.
    trust_pem: PathBuf,

    /// Path to certificate revocation list PEM (optional).
    ///
    /// Lists certificates that should no longer be trusted, even if they appear in the trust
    /// chain (e.g., compromised or retired peers).
    ///
    /// SECURITY: Keep this updated from your certificate authority to maintain security.
    crls_pem: Option<PathBuf>,

    /// Gateway addresses to connect to (required for network entry).
    ///
    /// Specifies one or more gateways for network bootstrapping and relay functionality.
    /// Multiple gateways improve redundancy; the inspector attempts to connect to all
    /// configured peers, succeeding if any are reachable.
    ///
    /// Examples:
    /// * "/ip4/203.0.113.10/tcp/8080/"
    /// * "/dns4/gateway.hypha.example/tcp/443/"
    ///
    /// NOTE: Defaults to placeholder addresses so users must configure real endpoints.
    #[serde(alias = "gateways")]
    gateway_addresses: Vec<Multiaddr>,

    /// Network tuning for QUIC transport (bandwidth, RTT, handshake timeout).
    ///
    /// These values size QUIC flow-control windows using the bandwidth-delay product
    /// and set the handshake timeout. Defaults target a 1 Gbps link with 100 ms RTT
    /// and a 30s handshake deadline.
    #[serde(default)]
    network: NetworkConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("inspect-cert.pem"),
            key_pem: PathBuf::from("inspect-key.pem"),
            trust_pem: PathBuf::from("inspect-trust.pem"),
            crls_pem: None,
            gateway_addresses: vec![
                "/ip4/1.2.3.4/tcp/1234"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/1.2.3.5/udp/1234/quic-v1"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
            ],
            network: NetworkConfig::default(),
        }
    }
}

impl Config {
    pub fn gateway_addresses(&self) -> &Vec<Multiaddr> {
        &self.gateway_addresses
    }

    pub fn network(&self) -> &NetworkConfig {
        &self.network
    }
}

impl TLSConfig for Config {
    fn cert_pem_path(&self) -> &std::path::Path {
        &self.cert_pem
    }

    fn key_pem_path(&self) -> &std::path::Path {
        &self.key_pem
    }

    fn trust_pem_path(&self) -> &std::path::Path {
        &self.trust_pem
    }

    fn crls_pem_path(&self) -> Option<&std::path::Path> {
        self.crls_pem.as_deref()
    }
}

impl ValidatableConfig for Config {
    fn validate(
        _cfg: &ConfigWithMetadata<Self>,
    ) -> std::result::Result<(), hypha_config::ConfigError> {
        Ok(())
    }
}
