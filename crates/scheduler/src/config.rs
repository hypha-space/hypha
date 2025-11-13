use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::{ConfigError, ConfigWithMetadata, TLSConfig, ValidatableConfig};
use hypha_network::{IpNet, find_containing_cidr, reserved_cidrs};
use hypha_telemetry::{
    attributes::Attributes,
    otlp::{Endpoint, Headers, Protocol},
    tracing::SamplerKind,
};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::scheduler_config::SchedulerConfig;

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Scheduler configuration for ML job orchestration and coordination.
///
/// The scheduler discovers worker nodes and orchestrates distributed ML training jobs.
/// It should be deployed with sufficient resources for coordination overhead.
pub struct Config {
    /// Path to the TLS certificate PEM file.
    ///
    /// Must be a valid X.509 certificate in PEM format that establishes this scheduler's
    /// identity in the P2P network. The certificate must match the private key and be
    /// trusted by all peers.
    ///
    /// SECURITY: Use certificates from a recognized CA or internal PKI for production deployments.
    cert_pem: PathBuf,

    /// Path to the private key PEM file.
    ///
    /// Must correspond to cert_pem. This is the scheduler's cryptographic identity.
    ///
    /// SECURITY:
    ///   * Restrict file permissions (chmod 600 recommended)
    ///   * Never commit to version control
    ///   * Store securely using secrets management systems in production
    ///   * Keep backups in secure, encrypted storage
    key_pem: PathBuf,

    /// Path to the trust chain PEM file (CA bundle).
    ///
    /// Contains root and intermediate certificates trusted by this scheduler. Peers presenting
    /// certificates signed by these CAs will be accepted for network connections.
    trust_pem: PathBuf,

    /// Path to certificate revocation list PEM (optional).
    ///
    /// Specifies certificates that should no longer be trusted, even if they're in the trust
    /// chain. Used for compromised certificates or decommissioned peers.
    ///
    /// SECURITY: Keep this updated with your certificate authority's latest CRL to maintain
    /// network security. Automate CRL updates in production environments.
    crls_pem: Option<PathBuf>,

    /// Gateway addresses to connect to (required for network entry).
    ///
    /// Specifies one or more gateways for network bootstrapping and relay functionality.
    ///
    /// Multiple gateways provide redundancy; the scheduler attempts to connect to all
    /// and succeeds if any are reachable.
    ///
    /// Examples:
    /// * "/ip4/203.0.113.10/tcp/8080/"
    /// * "/dns4/gateway.hypha.example/tcp/443/"
    gateway_addresses: Vec<Multiaddr>,

    /// Network addresses to listen on for incoming connections.
    ///
    /// Supports TCP and QUIC protocols. Use port 0 to let the OS assign available ports.
    ///
    /// Examples:
    /// * "/ip4/0.0.0.0/tcp/0" - TCP on all interfaces, OS-assigned port
    /// * "/ip4/0.0.0.0/udp/0/quic-v1" - QUIC on all interfaces, OS-assigned port
    listen_addresses: Vec<Multiaddr>,

    /// External addresses to advertise for peer discovery (optional).
    ///
    /// Only advertise addresses that workers can reliably reach. Most schedulers rely on
    /// relay circuits and don't need external addresses.
    ///
    /// Examples:
    /// * "/ip4/203.0.113.20/tcp/9090"
    /// * "/dns4/scheduler.example.com/tcp/9090"
    external_addresses: Vec<Multiaddr>,

    /// CIDR address filters for DHT routing table management.
    ///
    /// Peer addresses matching these CIDR ranges are excluded from the Kademlia DHT before
    /// being added. This prevents routing to non-routable or private addresses.
    ///
    /// Defaults to reserved/private ranges (loopback, RFC1918, etc.).
    ///
    /// Add additional ranges to filter internal addresses specific to your network topology.
    ///
    /// NOTE: This only affects DHT address filtering, not direct peer connections.
    #[serde(default = "reserved_cidrs")]
    exclude_cidr: Vec<IpNet>,

    /// Enable listening via relay circuit through the gateway.
    ///
    /// When enabled (default), the scheduler establishes a listening address via the gateway's
    /// relay circuit (/p2p-circuit). This allows workers to reach the scheduler even if it's
    /// behind NAT or firewall.
    ///
    /// RECOMMENDATION: Keep enabled (true) unless the scheduler has public IP and external
    /// addresses configured for direct connectivity.
    relay_circuit: bool,

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
    /// Key-value pairs that identify this scheduler instance in your observability platform.
    /// Useful for filtering and grouping metrics across multiple schedulers.
    ///
    /// Example Attributes:
    /// * service.name: "hypha-scheduler"
    /// * service.version: "0.1.0"
    /// * deployment.environment: "production"
    /// * host.name: "scheduler-01"
    /// * job.type: "training"
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
    /// observability with costs. Start with 0.1 (10%) and adjust based on job volume.
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
    /// significance. For high-volume schedulers, 0.01-0.1 is probably sufficient.
    #[serde(alias = "traces_sampler_arg")]
    telemetry_sample_ratio: Option<f64>,

    /// AIM relay server address for real-time training metrics (optional).
    ///
    /// Connects to an AIM server to stream training metrics in real-time. Useful for
    /// monitoring job progress and visualizing training curves.
    ///
    /// Example: "0.0.0.0:61000"
    status_bridge: Option<String>,

    /// Scheduler-specific configuration for job orchestration.
    ///
    /// Contains settings for resource allocation, job scheduling policies, and worker
    /// management strategies.
    scheduler: SchedulerConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("scheduler-cert.pem"),
            key_pem: PathBuf::from("scheduler-key.pem"),
            trust_pem: PathBuf::from("scheduler-trust.pem"),
            crls_pem: None,
            // NOTE: Placeholder gateway addresses so users must configure real endpoints.
            gateway_addresses: vec![
                "/ip4/1.2.3.4/tcp/1234"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/1.2.3.5/udp/1234/quic-v1"
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
            exclude_cidr: reserved_cidrs(),
            // NOTE: Enabled by default to support inbound connectivity via relays
            // when behind NAT or firewall.
            relay_circuit: true,
            telemetry_attributes: None,
            telemetry_endpoint: None,
            telemetry_headers: None,
            telemetry_protocol: None,
            telemetry_sampler: None,
            telemetry_sample_ratio: None,
            status_bridge: Some("0.0.0.0:61000".to_string()),
            scheduler: SchedulerConfig::default(),
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
    pub fn exclude_cidr(&self) -> &Vec<IpNet> {
        &self.exclude_cidr
    }

    /// Whether to listen via a relay P2pCircuit through the gateway.
    pub fn relay_circuit(&self) -> bool {
        self.relay_circuit
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

    pub fn status_bridge(&self) -> Option<String> {
        self.status_bridge.clone()
    }

    pub fn scheduler_config(&self) -> &SchedulerConfig {
        &self.scheduler
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
    fn validate(cfg: &ConfigWithMetadata<Self>) -> std::result::Result<(), ConfigError> {
        if let Some((address, cidr)) = cfg
            .gateway_addresses()
            .iter()
            .find_map(|addr| find_containing_cidr(addr, cfg.exclude_cidr()).map(|c| (addr, c)))
        {
            let metadata = cfg.find_metadata("exclude_cidr");
            let message = format!("Gateway address `{address}` overlaps excluded CIDR `{cidr}`.");

            return Err(ConfigError::with_metadata(&metadata)(ConfigError::Invalid(
                message,
            )));
        }

        Ok(())
    }
}
