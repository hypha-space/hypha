use std::path::PathBuf;

use clap::{Parser, Subcommand};
use indoc::indoc;
use libp2p::{Multiaddr, PeerId};
use serde::Serialize;

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-inspect",
    version,
    about = "Hypha Inspect - Network Diagnostics and Introspection Tool",
    long_about = indoc!{"
        Hypha Inspector is a CLI tool for diagnosing network connectivity,
        inspecting routing tables, and verifying identities within the Hypha network.
    "}
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
pub enum Commands {
    #[command(
        about = "Check if a remote peer is healthy and reachable",
        long_about = indoc!{"
            Connects to the specified multiaddr and performs a health check.

            In verbose mode, prints detailed routing information and connection stats.
        "}
    )]
    #[serde(untagged)]
    Probe {
        /// Path to the configuration file
        #[arg(
            short,
            long("config"),
            default_value = "config.toml",
            verbatim_doc_comment
        )]
        config_file: PathBuf,

        /// Target peer multiaddr to probe
        #[arg(index = 1, verbatim_doc_comment)]
        address: String,

        /// Maximum time to wait for health response (milliseconds)
        ///
        /// If the peer doesn't respond within this duration, the probe fails.
        /// Increase for high-latency networks or overloaded peers.
        #[arg(long, default_value_t = 2000, verbatim_doc_comment)]
        timeout: u64,

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
    },

    #[command(
        about = "Lookup a peer in the DHT via gateways",
        long_about = indoc!{"
            Connects to configured gateways and performs a DHT lookup for the given PeerID.
            Prints the routing table information (closest peers, addresses) found.
        "}
    )]
    #[serde(untagged)]
    Lookup {
        /// The PeerID to lookup
        #[arg(index = 1)]
        peer_id: PeerId,

        /// Path to the configuration file
        #[arg(
            short,
            long("config"),
            default_value = "config.toml",
            verbatim_doc_comment
        )]
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
    },

    #[command(
        about = "Derive PeerID from a certificate file",
        long_about = indoc!{"
            Reads an X.509 certificate or private key PEM file and prints the corresponding
            libp2p PeerID.
        "}
    )]
    #[serde(untagged)]
    CertInfo {
        /// Path to the PEM file (certificate or private key)
        #[arg(index = 1, verbatim_doc_comment)]
        path: PathBuf,
    },
    #[command(
        about = "Generate a default configuration file",
        long_about = indoc!{"
            Generate a default configuration file

            Creates a TOML configuration file with sensible defaults.
        "}
    )]
    #[serde(untagged)]
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml", verbatim_doc_comment)]
        output: PathBuf,

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
    },
}
