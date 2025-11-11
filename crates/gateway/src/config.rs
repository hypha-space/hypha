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
    /// Addresses to listen on.
    listen_addresses: Vec<Multiaddr>,
    /// External addresses to advertise. Only list addresses that are guaranteed to be reachable from the internet.
    external_addresses: Vec<Multiaddr>,
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
    /// CIDR address filters applied before adding Identify-reported listen addresses to Kademlia.
    ///
    /// Use standard CIDR notation (e.g., "10.0.0.0/8", "fc00::/7"). Defaults to loopback addresses.
    #[serde(default = "reserved_cidrs")]
    exclude_cidr: Vec<IpNet>,
}

impl Config {
    /// Create a default configuration with the specified name prefix for certificate files.
    ///
    /// NOTE: This method enables the CLI `init --name` functionality by generating
    /// configuration files with customized certificate file names based on the provided name.
    /// This is essential for multi-node deployments where each node needs distinct certificates.
    pub fn with_name(name: &str) -> Self {
        Self {
            cert_pem: PathBuf::from(format!("{}-cert.pem", name)),
            key_pem: PathBuf::from(format!("{}-key.pem", name)),
            trust_pem: PathBuf::from(format!("{}-trust.pem", name)),
            crls_pem: None,
            listen_addresses: vec![
                "/ip4/127.0.0.1/tcp/8080"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/127.0.0.1/udp/8080/quic-v1"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
            ],
            external_addresses: vec![
                "/ip4/127.0.0.1/tcp/8080"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/127.0.0.1/udp/8080/quic-v1"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
            ],
            telemetry_attributes: None,
            telemetry_endpoint: None,
            telemetry_headers: None,
            telemetry_protocol: None,
            telemetry_sampler: None,
            telemetry_sample_ratio: None,
            exclude_cidr: reserved_cidrs(),
        }
    }
}

impl Config {
    pub fn listen_addresses(&self) -> &Vec<Multiaddr> {
        &self.listen_addresses
    }

    pub fn external_addresses(&self) -> &Vec<Multiaddr> {
        &self.external_addresses
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

    pub fn exclude_cidr(&self) -> &Vec<IpNet> {
        &self.exclude_cidr
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_with_name_sets_certificate_file_names() {
        let config = Config::with_name("test-gateway");

        assert_eq!(config.cert_pem, PathBuf::from("test-gateway-cert.pem"));
        assert_eq!(config.key_pem, PathBuf::from("test-gateway-key.pem"));
        assert_eq!(config.trust_pem, PathBuf::from("test-gateway-trust.pem"));
    }

    #[test]
    fn config_with_name_preserves_other_default_values() {
        let config = Config::with_name("custom");

        // Verify that non-certificate fields retain their default values
        assert_eq!(config.crls_pem, None);
        assert!(!config.listen_addresses.is_empty());
        assert!(!config.external_addresses.is_empty());
    }

    #[test]
    fn config_with_name_handles_empty_string() {
        let config = Config::with_name("");

        assert_eq!(config.cert_pem, PathBuf::from("-cert.pem"));
        assert_eq!(config.key_pem, PathBuf::from("-key.pem"));
        assert_eq!(config.trust_pem, PathBuf::from("-trust.pem"));
    }

    #[test]
    fn config_with_name_handles_special_characters() {
        let config = Config::with_name("gateway-123_test");

        assert_eq!(config.cert_pem, PathBuf::from("gateway-123_test-cert.pem"));
        assert_eq!(config.key_pem, PathBuf::from("gateway-123_test-key.pem"));
        assert_eq!(
            config.trust_pem,
            PathBuf::from("gateway-123_test-trust.pem")
        );
    }
}
