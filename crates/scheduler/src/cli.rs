use std::path::PathBuf;

use clap::{Parser, Subcommand};
use hypha_network::IpNet;
use indoc::indoc;
use libp2p::Multiaddr;
use serde::Serialize;

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-scheduler",
    version,
    about = "Hypha Scheduler - ML Job Orchestration",
    long_about = indoc!{"
        The Hypha Scheduler discovers workers via the Hypha network and orchestrates
        distributed ML training jobs.
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

            Creates a TOML configuration file with sensible defaults for job orchestration,
            including certificate paths, network addresses, gateway connections, and job settings.

            IMPORTANT: If the output file exists, it will be overwritten without warning.
        "}
    )]
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml", verbatim_doc_comment)]
        output: PathBuf,
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
            * Monitoring scheduler availability
            * Deployment verification and readiness checks

            NOTE: It's not possible to self-probe using the same certificate used to run the scheduler.
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

        /// Maximum time to wait for health response (milliseconds)
        ///
        /// If the peer doesn't respond within this duration, the probe fails.
        /// Increase for high-latency networks or overloaded peers.
        #[arg(long, default_value_t = 2000, verbatim_doc_comment)]
        timeout: u64,

        /// Target peer multiaddr to probe
        ///
        /// Examples:
        ///   /ip4/192.168.1.100/tcp/8080/
        ///   /dns4/scheduler.example.com/tcp/443/p2p/12D3KooW...
        #[arg(index = 1, verbatim_doc_comment)]
        address: String,
    },

    #[command(
        about = "Start the scheduler and begin job orchestration",
        long_about = indoc!{"
            Start the scheduler and begin job orchestration

            Loads configuration, connects to gateways, discovers workers, and orchestrates
            training jobs. Runs until interrupted (SIGINT/SIGTERM) with a graceful shutdown.
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
        ///
        /// Examples:
        ///   --gateway /ip4/203.0.113.10/tcp/8080/
        ///   --gateway /dns4/gateway.hypha.example/tcp/443/
        #[arg(long("gateway"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (repeatable, overrides config)
        ///
        /// Where the scheduler accepts incoming connections.
        ///
        /// Examples:
        ///   --listen /ip4/0.0.0.0/tcp/9090
        ///   --listen /ip4/0.0.0.0/udp/9090/quic-v1
        #[arg(long("listen"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,

        /// External addresses to advertise (repeatable, overrides config)
        ///
        /// Publicly reachable addresses peers should use to connect.
        ///
        /// Examples:
        ///   --external /ip4/203.0.113.20/tcp/9090
        ///   --external /dns4/scheduler.example.com/tcp/9090
        #[arg(long("external"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        external_addresses: Option<Vec<Multiaddr>>,

        /// Enable relay circuit listening via gateway (overrides config)
        ///
        /// true = use relay (default), false = direct connections only.
        #[arg(long("relay-circuit"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_circuit: Option<bool>,

        /// CIDR ranges to exclude from DHT (repeatable, overrides config)
        ///
        /// Filters out peer addresses matching these ranges before adding to the DHT.
        /// Examples: 10.0.0.0/8, fc00::/7
        #[arg(long("exclude-cidr"), verbatim_doc_comment)]
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_cidr: Option<Vec<IpNet>>,
    },
}
