use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::{ConfigError, ConfigWithMetadata, TLSConfig, ValidatableConfig};
use hypha_network::{IpNet, reserved_cidrs};
use hypha_telemetry::{
    attributes::Attributes,
    otlp::{Endpoint, Headers, Protocol},
    tracing::SamplerKind,
};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Gateway configuration for network entry point and relay functionality.
///
/// The gateway serves as a stable, publicly-accessible entry point for the Hypha P2P network.
/// It must be deployed on a machine with public IP address and stable connectivity.
pub struct Config {
    /// Path to the TLS certificate PEM file.
    ///
    /// Must be a valid X.509 certificate in PEM format that establishes this gateway's identity
    /// in the P2P network. The certificate must match the private key and be trusted by all peers.
    ///
    /// SECURITY: Use certificates from a recognized CA or internal PKI for production deployments.
    /// For testing, self-signed certificates are acceptable but must be distributed to all peers.
    cert_pem: PathBuf,

    /// Path to the private key PEM file.
    ///
    /// Must correspond to cert_pem. This is the gateway's cryptographic identity.
    ///
    /// SECURITY:
    ///   * Restrict file permissions (chmod 600 recommended)
    ///   * Never commit to version control
    ///   * Store securely using secrets management systems in production
    ///   * Keep backups in secure, encrypted storage
    key_pem: PathBuf,

    /// Path to the trust chain PEM file (CA bundle).
    ///
    /// Contains root and intermediate certificates trusted by this gateway. Peers presenting
    /// certificates signed by these CAs will be accepted for network connections.
    ///
    /// For self-signed deployments, include all peer certificates. For production, use
    /// certificates from your organization's PKI or a recognized CA.
    trust_pem: PathBuf,

    /// Path to certificate revocation list PEM (optional).
    ///
    /// Specifies certificates that should no longer be trusted, even if they're in the trust
    /// chain. Used for compromised certificates or decommissioned peers.
    ///
    /// SECURITY: Keep this updated with your certificate authority's latest CRL to maintain
    /// network security. Automate CRL updates in production environments.
    crls_pem: Option<PathBuf>,

    /// Network addresses to listen on for incoming connections.
    ///
    /// Supports TCP and QUIC protocols. Use 0.0.0.0 to bind to all interfaces, or specify
    /// particular IPs to restrict to certain interfaces.
    ///
    /// Examples:
    /// * "/ip4/0.0.0.0/tcp/8080" - TCP on all interfaces
    /// * "/ip4/0.0.0.0/udp/8080/quic-v1" - QUIC on all interfaces
    /// * "/ip6/::/tcp/8080" - TCP on all IPv6 interfaces
    listen_addresses: Vec<Multiaddr>,

    /// External addresses to advertise for peer discovery.
    ///
    /// IMPORTANT: Only list addresses that are guaranteed to be reachable from the public
    /// internet. These addresses are shared with all peers for network connectivity.
    ///
    /// REQUIREMENTS:
    /// * Must be publicly routable IP addresses or resolvable DNS names
    /// * Must have proper port forwarding configured if behind NAT
    /// * Should be stable and consistently available
    /// * At least one external address required for network to function
    ///
    /// Examples:
    /// * "/ip4/203.0.113.10/tcp/8080" - Public IPv4 address
    /// * "/dns4/gateway.example.com/tcp/8080" - DNS name with public IP
    external_addresses: Vec<Multiaddr>,

    /// OpenTelemetry Protocol (OTLP) endpoint for exporting telemetry data.
    ///
    /// Sends metrics, traces, and logs to an OpenTelemetry collector or compatible backend
    /// (e.g., Jaeger, Prometheus, Grafana Cloud, ...).
    ///
    /// If unset, telemetry export is disabled (local logging only).
    #[serde(alias = "exporter_otlp_endpoint")]
    telemetry_endpoint: Option<Endpoint>,

    /// Resource attributes included in all telemetry data.
    ///
    /// Key-value pairs that identify this gateway instance in your observability platform.
    /// Useful for filtering and grouping metrics across multiple gateways.
    ///
    /// Example Attributes:
    /// * service.name: "hypha-gateway"
    /// * service.version: "0.1.0"
    /// * deployment.environment: "production"
    /// * host.name: "gateway-01"
    /// * cloud.provider: "aws"
    /// * cloud.region: "us-east-1"
    ///
    /// These attributes appear in all exported metrics, traces, and logs.
    #[serde(alias = "resource_attributes")]
    telemetry_attributes: Option<Attributes>,

    /// HTTP/gRPC headers for OTLP endpoint authentication.
    ///
    /// Used to authenticate with your telemetry backend. Common use cases:
    /// * API keys: {"Authorization": "Bearer YOUR_API_KEY"}
    /// * Custom headers: {"X-API-Key": "secret"}
    ///
    /// SECURITY: Protect these credentials. Use environment variables or secrets management
    /// instead of hardcoding in config files. Never commit credentials to version control.
    #[serde(alias = "exporter_otlp_headers")]
    telemetry_headers: Option<Headers>,

    /// Protocol for OTLP telemetry endpoint communication.
    ///
    /// Choose based on your collector's supported protocols.
    #[serde(alias = "exporter_otlp_protocol")]
    telemetry_protocol: Option<Protocol>,

    /// Trace sampling strategy to control volume and costs.
    ///
    /// Options:
    /// * "always_on" - Sample every trace (high volume, expensive)
    /// * "always_off" - Disable tracing (metrics and logs only)
    /// * "traceidratio" - Sample traces by probability (cost-effective)
    /// * "parentbased_traceidratio" - Honor parent trace decisions with fallback ratio
    ///
    /// RECOMMENDATION: Use "traceidratio" with sample_ratio for production to balance
    /// observability with costs. Start with 0.1 (10%) and adjust based on traffic volume.
    #[serde(alias = "traces_sampler")]
    telemetry_sampler: Option<SamplerKind>,

    /// Sampling probability for ratio-based trace samplers.
    ///
    /// Valid range: 0.0 to 1.0
    /// * 1.0 = 100% sampling (sample every trace)
    /// * 0.1 = 10% sampling (sample 1 in 10 traces)
    /// * 0.01 = 1% sampling (sample 1 in 100 traces)
    ///
    /// Only applies to "traceidratio" and "parentbased_traceidratio" samplers.
    ///
    /// NOTE: Lower ratios reduce telemetry costs while maintaining statistical
    /// significance. For high-traffic gateways, 0.01-0.1 is typically sufficient.
    #[serde(alias = "traces_sampler_arg")]
    telemetry_sample_ratio: Option<f64>,

    /// CIDR address filters for DHT routing table management.
    ///
    /// Peer addresses matching these CIDR ranges are excluded from the Kademlia DHT before
    /// being added. This prevents routing to non-routable or private addresses.
    ///
    /// Defaults to reserved/private ranges:
    /// * 127.0.0.0/8, ::1/128 (loopback)
    /// * 10.0.0.0/8, 172.16.0.0/12, 192.168.0.0/16 (RFC1918 private)
    /// * fc00::/7 (IPv6 unique local)
    ///
    /// Add additional ranges to filter internal addresses specific to your network topology.
    ///
    /// Note: This only affects DHT address filtering, not direct peer connections.
    #[serde(default = "reserved_cidrs")]
    exclude_cidr: Vec<IpNet>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("gateway-cert.pem"),
            key_pem: PathBuf::from("gateway-key.pem"),
            trust_pem: PathBuf::from("gateway-trust.pem"),
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
