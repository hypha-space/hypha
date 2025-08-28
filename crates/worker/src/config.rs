use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::TLSConfig;
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::resources::ComputeResources;

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Configure available resources.
pub struct ResourceConfig {
    /// Available CPU cores.
    cpu: u32,
    /// Available memory in GB.
    memory: u32,
    /// Available storage in GB.
    storage: u32,
    // TODO: How do we want to map multiple GPUs?
    /// Available GPU memory in GB.
    gpu: u32,
}

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Configure network settings, security certificates, and runtime parameters.
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
    /// External addresses to advertise. Only list addresses that are guaranteed to be reachable from the internet.
    external_addresses: Vec<Multiaddr>,
    /// Enable listening via relay P2pCircuit through the gateway.
    /// Default is true to ensure inbound connectivity via relays.
    relay_circuit: bool,
    /// Available resources.
    resources: ResourceConfig,
    /// Available driver.
    driver: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("worker-cert.pem"),
            key_pem: PathBuf::from("worker-key.pem"),
            trust_pem: PathBuf::from("worker-trust.pem"),
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
            external_addresses: vec![],
            // NOTE: Enabled by default to support inbound connectivity via relays
            // when behind NAT or firewall.
            relay_circuit: true,
            resources: ResourceConfig::default(),
            driver: vec!["diloco-transformer".into()],
        }
    }
}

impl Default for ResourceConfig {
    fn default() -> Self {
        Self {
            cpu: 1,
            memory: 8,
            storage: 20,
            gpu: 16,
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

    pub fn external_addresses(&self) -> &Vec<Multiaddr> {
        &self.external_addresses
    }

    /// Whether to listen via a relay P2pCircuit through the gateway.
    pub fn relay_circuit(&self) -> bool {
        self.relay_circuit
    }

    pub fn resources(&self) -> ComputeResources {
        ComputeResources {
            cpu: self.resources.cpu.into(),
            gpu: self.resources.gpu.into(),
            memory: self.resources.memory.into(),
            storage: self.resources.storage.into(),
        }
    }

    pub fn driver(&self) -> Vec<String> {
        self.driver.clone()
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
