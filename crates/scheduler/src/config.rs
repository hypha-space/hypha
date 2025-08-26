use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::TLSConfig;
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Configure scheduler network settings, security certificates, and runtime parameters.
pub struct Config {
    /// Path to the certificate pem.
    cert_pem: PathBuf,
    /// Path to the private key pem.
    key_pem: PathBuf,
    /// Path to the trust pem (bundle).
    trust_pem: PathBuf,
    /// Path to the certificate revocation list pem.
    crls_pem: Option<PathBuf>,
    /// Addresses of the gateways.
    gateway_addresses: Vec<Multiaddr>,
    /// Addresses to listen on.
    listen_addresses: Vec<Multiaddr>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("scheduler-cert.pem"),
            key_pem: PathBuf::from("scheduler-key.pem"),
            trust_pem: PathBuf::from("scheduler-trust.pem"),
            crls_pem: None,
            gateway_addresses: vec![
                "/ip4/127.0.0.1/tcp/8080"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/127.0.0.1/udp/8080/quic-v1"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
            ],
            listen_addresses: vec![
                "/ip4/127.0.0.1/tcp/0"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/127.0.0.1/udp/0/quic-v1"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
            ],
        }
    }
}

impl Config {
    pub fn gateway_addresses(&self) -> &Vec<Multiaddr> {
        &self.gateway_addresses
    }

    pub fn listen_addresses(&self) -> &Vec<Multiaddr> {
        &self.listen_addresses
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
