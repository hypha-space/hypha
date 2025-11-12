use std::path::PathBuf;

use clap::{Parser, Subcommand};
use indoc::indoc;
use serde::Serialize;

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-data",
    version,
    about = "Hypha Data - Dataset Provider",
    long_about = indoc!{"
        The Hypha Data serves datasets to workers via the Hypha network.

        Data peers connect to gateways, announce their datasets via the DHT, and respond
        to data fetch requests from workers during training. Each dataset is divided into
        slices (files) that can be distributed across multiple workers for parallel processing.
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

            Creates a TOML configuration file with sensible defaults for dataset serving.
            The generated config includes certificate paths, network addresses, gateway
            connections, and dataset path configuration.

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
            with code 0 if the peer responds as healthy, or non-zero otherwise.

            Useful for:
            * Verifying gateway connectivity before announcing datasets
            * Container health checks (Docker, Kubernetes)
            * Monitoring data node availability
            * Deployment verification and readiness checks

            NOTE: It's not possible to self-probe using the same certificate used to run the data node.
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
        ///   /dns4/data.example.com/tcp/443/p2p/12D3KooW...
        #[arg(index = 1, verbatim_doc_comment)]
        address: String,
    },

    #[command(
        about = "Start the data node and begin serving datasets",
        long_about = indoc!{"
            Start the data node and begin serving datasets

            Loads configuration, connects to gateways, and enters the main serving loop.
            The process runs until interrupted (SIGINT/SIGTERM), then performs graceful
            shutdown to ensure data transfers are properly terminated.
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
    },
}
