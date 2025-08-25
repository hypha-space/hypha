use std::{net::SocketAddr, path::PathBuf};

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::TLSConfig;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Configure gateway network settings, security certificates, and runtime parameters.
pub struct Config {
    /// Path to the certificate pem.
    cert_pem: PathBuf,
    /// Path to the private key pem.
    key_pem: PathBuf,
    /// Path to the trust pem (bundle).
    trust_pem: PathBuf,
    /// Path to the certificate revocation list pem.
    crls_pem: Option<PathBuf>,
    /// Address to listen on.
    listen_address: SocketAddr,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("gateway-cert.pem"),
            key_pem: PathBuf::from("gateway-key.pem"),
            trust_pem: PathBuf::from("gateway-trust.pem"),
            crls_pem: None,
            listen_address: SocketAddr::from(([127, 0, 0, 1], 8080)),
        }
    }
}

impl Config {
    pub fn listen_address(&self) -> &SocketAddr {
        &self.listen_address
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
