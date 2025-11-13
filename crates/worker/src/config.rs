use std::path::PathBuf;

use documented::{Documented, DocumentedFieldsOpt};
use hypha_config::{ConfigError, ConfigWithMetadata, TLSConfig, ValidatableConfig};
use hypha_messages::ExecutorDescriptor;
use hypha_network::{IpNet, find_containing_cidr, reserved_cidrs};
use hypha_telemetry::{
    attributes::Attributes,
    otlp::{Endpoint, Headers, Protocol},
    tracing::SamplerKind,
};
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

use crate::resources::ComputeResources;

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Available compute resources advertised to schedulers for job allocation.
///
/// Resources are reserved during job allocation. Configure conservatively to avoid
/// overcommitment and ensure jobs have sufficient resources to complete successfully.
pub struct ResourceConfig {
    /// Available CPU cores.
    ///
    /// Number of CPU cores available for job execution. Jobs can request specific CPU
    /// allocations, and the scheduler ensures total allocated CPU doesn't exceed this limit.
    cpu: u32,

    /// Available memory in GB.
    ///
    /// Total system memory available for jobs. Configure based on actual RAM minus OS overhead
    /// and other running processes. Jobs exceeding this limit may be killed by the OS.
    memory: u32,

    /// Available storage in GB.
    ///
    /// Disk space available for job artifacts (models, checkpoints, datasets). Should match
    /// the available space in work_dir. Jobs with large datasets or frequent checkpoints
    /// require more storage.
    storage: u32,

    /// Available GPU memory in GB.
    ///
    /// Total GPU memory available across all GPUs. For multi-GPU systems, this represents
    /// the combined memory. Set to 0 if no GPU is available.
    ///
    /// NOTE: Current implementation treats this as aggregate GPU memory. Future versions
    /// may support per-GPU resource tracking for heterogeneous GPU clusters.
    gpu: u32,
}

#[derive(Clone, Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Executor configuration defining available job types and their runtime implementations.
///
/// Workers advertise executors to schedulers. Each executor specifies a class (e.g., "trainer",
/// "parameter-server") and implementation details. Schedulers match jobs to workers based on
/// required executor classes.
pub struct ExecutorConfig {
    /// Executor descriptor exposed to the scheduler (class + name).
    ///
    /// The class identifies the type of executor (e.g., "trainer", "inference-engine",
    /// "parameter-server"). The name is a human-readable identifier for this specific
    /// executor instance.
    #[serde(flatten)]
    descriptor: ExecutorDescriptor,

    /// Runtime implementation used to fulfill this executor.
    ///
    /// Defines how jobs are executed: built-in implementations or external processes.
    #[serde(flatten)]
    runtime: ExecutorRuntime,
}

#[derive(Clone, Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
#[serde(tag = "runtime", rename_all = "kebab-case")]
/// Runtime implementation backing an executor.
///
/// Determines how the worker executes jobs assigned to this executor.
pub enum ExecutorRuntime {
    /// Built-in parameter server implementation.
    ///
    /// Uses the worker's native parameter server for distributed training coordination.
    /// No external process required.
    ParameterServer,

    /// External process launched by the worker.
    ///
    /// The worker launches and manages an external executable to handle job execution.
    /// Useful for custom training frameworks or specialized inference engines.
    Process {
        /// Command to execute for process-based executors.
        ///
        /// Path to the executable or command name (must be in PATH). The worker spawns
        /// this process for each job assigned to this executor.
        ///
        /// Examples:
        /// * "python" - Python interpreter
        /// * "/usr/local/bin/custom-trainer" - Custom training binary
        /// * "docker" - Container-based execution
        cmd: String,

        /// Arguments passed to the process-based executor.
        ///
        /// Command-line arguments appended when spawning the executor process. Job-specific
        /// parameters are typically passed via environment variables or standard input.
        ///
        /// Examples:
        /// * ["-m", "torch.distributed.run"] - PyTorch distributed launcher
        /// * ["--config", "/etc/trainer/config.yaml"] - Config file path
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
}

impl ExecutorConfig {
    pub fn descriptor(&self) -> ExecutorDescriptor {
        self.descriptor.clone()
    }

    pub fn runtime(&self) -> &ExecutorRuntime {
        &self.runtime
    }

    pub fn cmd(&self) -> Option<&str> {
        match &self.runtime {
            ExecutorRuntime::Process { cmd, .. } => Some(cmd.as_str()),
            ExecutorRuntime::ParameterServer => None,
        }
    }

    pub fn args(&self) -> &[String] {
        match &self.runtime {
            ExecutorRuntime::Process { args, .. } => args,
            ExecutorRuntime::ParameterServer => &[],
        }
    }
}

#[derive(Deserialize, Serialize, Documented, DocumentedFieldsOpt)]
/// Worker configuration for ML job execution and resource management.
///
/// The worker executes ML training and inference jobs assigned by schedulers. It should be
/// deployed with adequate compute resources (CPU, GPU, memory, storage) for target workloads.
pub struct Config {
    /// Path to the TLS certificate PEM file.
    ///
    /// Must be a valid X.509 certificate in PEM format that establishes this worker's
    /// identity in the P2P network. The certificate must match the private key and be
    /// trusted by all peers.
    ///
    /// SECURITY: Use certificates from a recognized CA or internal PKI for production deployments.
    cert_pem: PathBuf,

    /// Path to the private key PEM file.
    ///
    /// Must correspond to cert_pem. This is the worker's cryptographic identity.
    ///
    /// SECURITY:
    ///   * Restrict file permissions (chmod 600 recommended)
    ///   * Never commit to version control
    ///   * Store securely using secrets management systems in production
    ///   * Keep backups in secure, encrypted storage
    key_pem: PathBuf,

    /// Path to the trust chain PEM file (CA bundle).
    ///
    /// Contains root and intermediate certificates trusted by this worker. Peers presenting
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
    /// Multiple gateways provide redundancy; the worker attempts to connect to all
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
    /// Only advertise addresses that schedulers can reliably reach. Most workers rely on
    /// relay circuits and don't need external addresses.
    ///
    /// Examples:
    /// * "/ip4/203.0.113.30/tcp/9091"
    /// * "/dns4/worker.example.com/tcp/9091"
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
    /// When enabled (default), the worker establishes a listening address via the gateway's
    /// relay circuit (/p2p-circuit). This allows schedulers to reach the worker even if it's
    /// behind NAT or firewall.
    ///
    /// RECOMMENDATION: Keep enabled (true) unless the worker has public IP and external
    /// addresses configured for direct connectivity.
    relay_circuit: bool,

    /// Base directory for per-job working directories.
    ///
    /// Each job gets a unique subdirectory named hypha-{job_uuid} containing downloaded
    /// models, checkpoints, datasets, and job outputs.
    ///
    /// REQUIREMENTS:
    /// * Sufficient free space matching configured storage resource
    /// * Fast I/O performance (SSD strongly recommended for training)
    /// * Proper write permissions for worker process
    /// * Adequate space for concurrent jobs if running multiple
    ///
    /// Jobs clean up their directories on successful completion, but failures may leave
    /// artifacts for debugging.
    work_dir: PathBuf,

    /// Available compute resources advertised to schedulers.
    ///
    /// Accurately configure resources to enable proper job allocation. Resources are
    /// reserved during job allocation and jobs exceeding available resources will be rejected.
    resources: ResourceConfig,

    /// Available executors for different job types.
    ///
    /// Executors define the runtime implementations available on this worker. Configure
    /// executors for training, inference, parameter serving, or custom workloads.
    ///
    /// Workers with no executors configured cannot accept jobs.
    executors: Vec<ExecutorConfig>,

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
    /// Key-value pairs that identify this worker instance in your observability platform.
    /// Useful for filtering and grouping metrics across multiple workers.
    ///
    /// Example Attributes:
    /// * service.name: "hypha-worker"
    /// * service.version: "0.1.0"
    /// * deployment.environment: "production"
    /// * host.name: "worker-gpu-01"
    /// * hardware.gpu: "nvidia-a100"
    /// * resource.tier: "high-memory"
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
    /// significance. For high-throughput workers, 0.01-0.1 is probably sufficient.
    #[serde(alias = "traces_sampler_arg")]
    telemetry_sample_ratio: Option<f64>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            cert_pem: PathBuf::from("worker-cert.pem"),
            key_pem: PathBuf::from("worker-key.pem"),
            trust_pem: PathBuf::from("worker-trust.pem"),
            crls_pem: None,
            // NOTE: Placeholder gateway addresses so users must configure real endpoints.
            gateway_addresses: vec![
                "/ip4/1.2.3.4/tcp/1234"
                    .parse()
                    .expect("default address parses into a Multiaddr"),
                "/ip4/1.2.3.4/udp/1234/quic-v1"
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
            // NOTE: Default work directory base. Jobs create subdirs `hypha-{uuid}` under this path.
            work_dir: PathBuf::from("/tmp"),
            resources: ResourceConfig::default(),
            executors: Vec::new(),
            telemetry_attributes: None,
            telemetry_endpoint: None,
            telemetry_headers: None,
            telemetry_protocol: None,
            telemetry_sampler: None,
            telemetry_sample_ratio: None,
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

    pub fn exclude_cidr(&self) -> &Vec<IpNet> {
        &self.exclude_cidr
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

    pub fn executors(&self) -> &[ExecutorConfig] {
        &self.executors
    }

    /// Base directory for per-job working directories.
    pub fn work_dir(&self) -> &PathBuf {
        &self.work_dir
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
