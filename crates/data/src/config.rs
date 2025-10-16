use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::TLSConfig;
use hypha_network::{IpNet, reserved_cidrs};
use hypha_telemetry::{
    attributes::Attributes,
    otlp::{Endpoint, Headers, Protocol},
    tracing::SamplerKind,
};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

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
    /// Path or file providing a dataset.
    dataset_path: PathBuf,
    /// CIDR address filters applied before adding Identify-reported listen addresses to Kademlia.
    /// Use standard CIDR notation (e.g., "10.0.0.0/8", "fc00::/7").
    #[serde(default = "reserved_cidrs")]
    exclude_cidr: Vec<IpNet>,
    #[serde(alias = "exporter_otlp_endpoint")]
    /// OTLP Exporter endpoint for telemetry data. If unset, telemetry is disabled.
    telemetry_endpoint: Option<Endpoint>,
    #[serde(alias = "resource_attributes")]
    /// Attributes to be included in telemetry.
    telemetry_attributes: Option<Attributes>,
    #[serde(alias = "exporter_otlp_headers")]
    /// Headers for OTLP telemetry endpoint request used for authentication.
    telemetry_headers: Option<Headers>,
    #[serde(alias = "exporter_otlp_protocol")]
    /// Protocol for OTLP telemetry endpoint request.
    telemetry_protocol: Option<Protocol>,
    #[serde(alias = "traces_sampler")]
    /// Traces sampler: one of "always_on", "always_off", "traceidratio", or "parentbased_traceidratio".
    telemetry_sampler: Option<SamplerKind>,
    #[serde(alias = "traces_sampler_arg")]
    /// For `traceidratio` and `parentbased_traceidratio` samplers: Sampling probability in [0..1],
    /// e.g. "0.25". Default is 1.0.
    telemetry_sample_ratio: Option<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("data-cert.pem"),
            key_pem: PathBuf::from("data-key.pem"),
            trust_pem: PathBuf::from("data-trust.pem"),
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
            dataset_path: PathBuf::new(),
            exclude_cidr: reserved_cidrs(),
            telemetry_attributes: None,
            telemetry_endpoint: None,
            telemetry_headers: None,
            telemetry_protocol: None,
            telemetry_sampler: None,
            telemetry_sample_ratio: None,
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

    /// Base directory for per-job working directories.
    pub fn dataset_path(&self) -> &PathBuf {
        &self.dataset_path
    }

    pub fn exclude_cidr(&self) -> &Vec<IpNet> {
        &self.exclude_cidr
    }

    pub fn telemetry_endpoint(&self) -> Option<Endpoint> {
        self.telemetry_endpoint.clone()
    }

    pub fn telemetry_headers(&self) -> Option<Headers> {
        self.telemetry_headers.clone()
    }

    pub fn telemetry_attributes(&self) -> Option<Attributes> {
        self.telemetry_attributes.clone()
    }

    pub fn telemetry_protocol(&self) -> Option<Protocol> {
        self.telemetry_protocol
    }

    /// Optional trace sampling ratio (0.0–1.0). If set, used to configure the tracer sampler.
    pub fn telemetry_sample_ratio(&self) -> Option<f64> {
        self.telemetry_sample_ratio
    }

    /// Optional traces sampler name.
    pub fn telemetry_sampler(&self) -> Option<SamplerKind> {
        self.telemetry_sampler.clone()
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
