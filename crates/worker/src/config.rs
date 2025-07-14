use std::{net::SocketAddr, path::PathBuf};

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::TLSConfig;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Configure network settings, security certificates, and runtime parameters.
pub struct Config {
    /// Path to the certificate pem.
    cert_pem: PathBuf,
    /// Path to the private key pem.
    key_pem: PathBuf,
    /// Path to the trust chain pem.
    trust_pem: PathBuf,
    /// Path to the certificate revocation list pem.
    crls_pem: Option<PathBuf>,
    /// Address of the gateway.
    gateway_address: SocketAddr,
    /// Address to listen on.
    listen_address: SocketAddr,
    /// Path to the socket file.
    socket_path: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("worker-cert.pem"),
            key_pem: PathBuf::from("worker-key.pem"),
            trust_pem: PathBuf::from("worker-trust.pem"),
            crls_pem: None,
            gateway_address: SocketAddr::from(([127, 0, 0, 1], 8080)),
            listen_address: SocketAddr::from(([127, 0, 0, 1], 0)),
            socket_path: PathBuf::from("/var/run/hypha.sock"),
        }
    }
}

impl Config {
    pub fn gateway_address(&self) -> &SocketAddr {
        &self.gateway_address
    }

    pub fn listen_address(&self) -> &SocketAddr {
        &self.listen_address
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
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
