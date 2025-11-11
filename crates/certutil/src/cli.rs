use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "hypha-certutil")]
#[command(about = "Certificate utility for Hypha network", long_about = None)]
#[command(version)]
#[command(after_help = "For more information and examples, see the module documentation")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Generate a Root CA certificate
    ///
    /// This creates the root of your PKI hierarchy. In production, this would be
    /// stored securely and used rarely. For development, you typically create one
    /// root CA and reuse it across your test environment.
    #[command(after_help = "Example: hypha-certutil root -n 'Test Root CA' -d certs/root")]
    Root {
        /// Organization name
        #[arg(short = 'o', long)]
        organization: String,

        /// Country name (2-letter code)
        #[arg(long, default_value = "US")]
        country: String,

        /// Common name for the Root CA (defaults to "<org> CA")
        #[arg(short = 'n', long)]
        name: Option<String>,

        /// Directory to save the certificate and key files
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },
    /// Generate an Intermediate Organization CA certificate signed by Root CA
    ///
    /// Organization CAs represent tenants in the Hypha network. Each tenant gets
    /// their own CA certificate that can issue certificates for their nodes.
    /// This provides cryptographic isolation between tenants.
    #[command(
        after_help = "Example: hypha-certutil org --root-cert root-ca-cert.pem --root-key root-ca-key.pem -o acme-corp"
    )]
    Org {
        /// Root CA certificate file path
        #[arg(long)]
        root_cert: PathBuf,

        /// Root CA private key file path
        #[arg(long)]
        root_key: PathBuf,

        /// Organization/tenant name (e.g., acme-corp)
        #[arg(short = 'o', long)]
        organization: String,

        /// Common name for the Organization CA (defaults to "<org> CA")
        #[arg(short = 'n', long)]
        name: Option<String>,

        /// Directory to save the certificate and key files
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },
    /// Generate a certificate signed by a CA (intermediate or root)
    ///
    /// Node certificates are used by individual services and nodes in the network.
    /// They should typically be signed by an Organization CA, not the root CA directly.
    /// The certificate will include a trust file (bundle) for easy deployment.
    #[command(
        after_help = "Example: hypha-certutil node --ca-cert acme-ca-cert.pem --ca-key acme-ca-key.pem -n node1.acme.local -s node1.acme.local,*.acme.local"
    )]
    Node {
        /// CA certificate file path
        #[arg(long)]
        ca_cert: PathBuf,

        /// CA private key file path
        #[arg(long)]
        ca_key: PathBuf,

        /// Common name for the certificate (e.g., node1.acme-corp.hypha.network)
        #[arg(short = 'n', long)]
        name: String,

        /// Subject Alternative Names (SANs) - DNS names, IPs, etc.
        /// Format: comma-separated list of DNS names and IP addresses
        /// The common name will be automatically added if not present
        #[arg(short, long, value_delimiter = ',', default_value = "0.0.0.0")]
        san: Vec<String>,

        /// Directory to save the certificate and key files
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },
}
