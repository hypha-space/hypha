use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use hypha_network::IpNet;
use indoc::indoc;
use libp2p::Multiaddr;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, ValueEnum, Serialize, Deserialize)]
pub enum Role {
    Worker,
    ParameterServer,
}

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-worker",
    version,
    about = "Hypha Worker",
    long_about = indoc!{"
        The Hypha Worker executes training and inference jobs assigned by schedulers via
        the Hypha network.
    "}
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
pub enum Commands {
    #[command(
        about = "Generate a default configuration file",
        long_about = indoc!{"
            Generate a default configuration file

            Creates a TOML configuration file with sensible defaults for job execution,
            including certificate paths, network addresses, gateway connections, and
            resource/executor settings.

            IMPORTANT: If the output file exists, it will be overwritten without warning.
        "}
    )]
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml", verbatim_doc_comment)]
        output: PathBuf,

        /// Name of this worker node.
        #[clap(short, long)]
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
    },

    #[command(
        about = "Check if a remote peer is healthy and reachable",
        long_about = indoc!{"
            Check if a remote peer is healthy and reachable

            Connects to the specified multiaddr, sends a health check request, and exits
            with code 0 if the peer is healthy, or non-zero otherwise.

            Useful for:
            * Verifying gateway connectivity before starting jobs
            * Container health checks (Docker, Kubernetes)
            * Monitoring worker availability
            * Deployment verification and readiness checks

            NOTE: It's not possible to self-probe using the same certificate used to run the worker.
        "}
    )]
    #[serde(untagged)]
    Probe {
        /// Path to the configuration file
        ///
        /// Used to load TLS certificates for secure connection to the target peer.
        #[arg(
            short,
            long("config"),
            default_value = "config.toml",
            verbatim_doc_comment
        )]
        config_file: PathBuf,

        /// Path to the certificate PEM file (overrides config)
        ///
        /// Must be a valid X.509 certificate in PEM format. If not provided, uses
        /// cert_pem from the configuration file.
        #[arg(long("cert"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        cert_pem: Option<PathBuf>,

        /// Path to the private key PEM file (overrides config)
        ///
        /// Must correspond to the certificate. If not provided, uses key_pem from
        /// the configuration file.
        ///
        /// SECURITY: Ensure this file has restricted permissions (e.g., chmod 600).
        #[arg(long("key"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        key_pem: Option<PathBuf>,

        /// Path to the trust chain PEM file (overrides config)
        ///
        /// CA bundle containing certificates trusted by this node. If not provided,
        /// uses trust_pem from the configuration file.
        #[arg(long("trust"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        trust_pem: Option<PathBuf>,

        /// Path to the certificate revocation list PEM (overrides config)
        ///
        /// Optional CRL for rejecting compromised certificates. If not provided,
        /// uses crls_pem from the configuration file if present.
        #[arg(long("crls"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        crls_pem: Option<PathBuf>,

        /// Target peer multiaddr to probe
        ///
        /// Examples:
        ///   /ip4/192.168.1.100/tcp/8080/
        ///   /dns4/worker.example.com/tcp/443/p2p/12D3KooW...
        #[arg(index = 1, verbatim_doc_comment)]
        address: String,

        /// Maximum time to wait for health response (milliseconds)
        ///
        /// If the peer doesn't respond within this duration, the probe fails.
        /// Increase for high-latency networks or overloaded peers.
        #[arg(long, default_value_t = 2000, verbatim_doc_comment)]
        timeout: u64,
    },

    #[command(
        about = "Start the worker and begin accepting jobs",
        long_about = indoc!{"
            Start the worker and begin accepting jobs

            Loads configuration, connects to gateways, advertises resources, and executes
            assigned jobs. Runs until interrupted (SIGINT/SIGTERM) with a graceful shutdown.
        "}
    )]
    #[serde(untagged)]
    Run {
        /// Path to the configuration file
        #[arg(
            short,
            long("config"),
            default_value = "config.toml",
            verbatim_doc_comment
        )]
        #[serde(skip)]
        config_file: PathBuf,

        /// Gateway addresses to connect to (repeatable, overrides config)
        ///
        /// Gateways provide network bootstrapping, DHT access, and optional relay.
        /// Must include the peer ID in the multiaddr.
        ///
        /// Examples:
        ///   --gateway /ip4/203.0.113.10/tcp/8080/p2p/12D3KooWAbc...
        ///   --gateway /dns4/gateway.hypha.example/tcp/443/p2p/12D3KooWAbc...
        /// Required: connect to at least one gateway.
        #[arg(long("gateway"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (repeatable, overrides config)
        ///
        /// Where the worker accepts incoming connections.
        ///
        /// Examples:
        ///   --listen /ip4/0.0.0.0/tcp/9091
        ///   --listen /ip4/0.0.0.0/udp/9091/quic-v1
        #[arg(long("listen"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,

        /// External addresses to advertise (repeatable, overrides config)
        ///
        /// Publicly reachable addresses peers should use to connect.
        ///
        /// Examples:
        ///   --external /ip4/203.0.113.30/tcp/9091
        ///   --external /dns4/worker.example.com/tcp/9091
        #[arg(long("external"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        external_addresses: Option<Vec<Multiaddr>>,

        /// Enable relay circuit listening via gateway (overrides config)
        ///
        /// true = use relay (default), false = direct connections only.
        #[arg(long("relay-circuit"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_circuit: Option<bool>,

        /// Socket path for driver communication (overrides config)
        ///
        /// Unix domain socket for worker-executor communication (optional).
        #[arg(long("socket"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        socket_address: Option<PathBuf>,

        /// Base directory for job working directories (overrides config)
        ///
        /// Where per-job working directories are created.
        ///
        /// Examples:
        ///   --work-dir /tmp
        ///   --work-dir /mnt/fast-ssd/hypha
        #[arg(long("work-dir"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        work_dir: Option<PathBuf>,

        /// CIDR ranges to exclude from DHT (repeatable, overrides config)
        ///
        /// Filters out peer addresses matching these ranges before adding to the DHT.
        ///
        /// Examples: 10.0.0.0/8, fc00::/7
        #[arg(long("exclude-cidr"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_cidr: Option<Vec<IpNet>>,
    },
}
