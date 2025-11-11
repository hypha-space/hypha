use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use hypha_network::IpNet;
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
    about = "Hypha Worker Node",
    long_about = "Runs a Hypha Worker which executes jobs.",
    after_help = "For more information, see the project documentation."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
pub enum Commands {
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml")]
        output: PathBuf,
    },

    /// Probe a target multiaddr for readiness and exit 0 if healthy.
    #[serde(untagged)]
    Probe {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        config_file: PathBuf,

        /// Path to the certificate pem.
        #[clap(long("cert"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        cert_pem: Option<PathBuf>,

        /// Path to the private key pem.
        #[clap(long("key"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        key_pem: Option<PathBuf>,

        /// Path to the trust pem (bundle).
        #[clap(long("trust"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        trust_pem: Option<PathBuf>,

        /// Path to the certificate revocation list pem.
        #[clap(long("crls"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        crls_pem: Option<PathBuf>,

        /// Target multiaddr to probe (e.g., /ip4/127.0.0.1/tcp/8080)
        #[clap(index = 1)]
        address: String,

        /// Timeout in milliseconds
        #[clap(long, default_value_t = 2000)]
        timeout: u64,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
        config_file: PathBuf,

        /// Addresses of the gateways (can be specified multiple times).
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (can be specified multiple times).
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,

        /// External addresses to advertise (can be specified multiple times).
        #[clap(long("external"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        external_addresses: Option<Vec<Multiaddr>>,

        /// Enable listening via relay P2pCircuit (via gateway). Defaults to true.
        #[clap(long("relay-circuit"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        relay_circuit: Option<bool>,

        /// Socket to use for driver communication.
        #[clap(long("socket"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        socket_address: Option<PathBuf>,

        /// Base directory for per-job working directories (default: /tmp).
        /// Example: --work-dir /mnt/tmp
        #[clap(long("work-dir"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        work_dir: Option<PathBuf>,

        /// CIDR exclusion (repeatable). Overrides config if provided.
        /// Example: --exclude-cidr 10.0.0.0/8 --exclude-cidr fc00::/7
        #[clap(long("exclude-cidr"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        exclude_cidr: Option<Vec<IpNet>>,
    },
}
