use std::{fs, net::SocketAddr, path::PathBuf};

use documented::{Documented, DocumentedFieldsOpt};
use figment::Figment;
use hypha_config::{ConfigError, LayeredConfig};
use hypha_network::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
};
use serde::{Deserialize, Serialize};

use crate::resources::Resources;

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
    #[serde(skip)]
    _figment: Option<Figment>,
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
    /// Available resources.
    resources: ResourceConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            _figment: None,
            cert_pem: PathBuf::from("worker-cert.pem"),
            key_pem: PathBuf::from("worker-key.pem"),
            trust_pem: PathBuf::from("worker-trust.pem"),
            crls_pem: None,
            gateway_address: SocketAddr::from(([127, 0, 0, 1], 8080)),
            listen_address: SocketAddr::from(([127, 0, 0, 1], 8081)),
            socket_path: PathBuf::from("/var/run/hypha.sock"),
            resources: ResourceConfig::default(),
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

impl LayeredConfig for Config {
    fn with_figment(mut self, figment: Figment) -> Self {
        self._figment = Some(figment);
        self
    }

    fn figment(&self) -> &Option<Figment> {
        &self._figment
    }
}

impl Config {
    pub fn load_cert_chain(&self) -> Result<Vec<CertificateDer<'static>>, ConfigError> {
        let metadata = self.find_metadata("cert_pem");

        load_certs_from_pem(
            &fs::read(&self.cert_pem).map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    pub fn load_key(&self) -> Result<PrivateKeyDer<'static>, ConfigError> {
        let metadata = self.find_metadata("key_pem");

        load_private_key_from_pem(
            &fs::read(&self.key_pem).map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    pub fn load_trust_chain(&self) -> Result<Vec<CertificateDer<'static>>, ConfigError> {
        let metadata = self.find_metadata("trust_pem");

        load_certs_from_pem(
            &fs::read(&self.trust_pem).map_err(ConfigError::with_metadata(&metadata))?,
        )
        .map_err(ConfigError::with_metadata(&metadata))
    }

    pub fn load_crls(&self) -> Result<Vec<CertificateRevocationListDer<'static>>, ConfigError> {
        let metadata = self.find_metadata("crls_pem");

        if let Some(crl_file) = &self.crls_pem {
            return load_crls_from_pem(
                &fs::read(crl_file).map_err(ConfigError::with_metadata(&metadata))?,
            )
            .map_err(ConfigError::with_metadata(&metadata));
        }

        Ok(vec![])
    }

    pub fn gateway_address(&self) -> &SocketAddr {
        &self.gateway_address
    }

    pub fn listen_address(&self) -> &SocketAddr {
        &self.listen_address
    }

    pub fn socket_path(&self) -> &PathBuf {
        &self.socket_path
    }

    pub fn resources(&self) -> Resources {
        Resources {
            cpu: self.resources.cpu.into(),
            gpu: self.resources.gpu.into(),
            memory: self.resources.memory.into(),
            storage: self.resources.storage.into(),
        }
    }
}
