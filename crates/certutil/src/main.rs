//! # Hypha Certificate Utility
//!
//! A certificate generation tool for the Hypha network that creates a three-tier PKI hierarchy:
//! - Root CA (central authority)
//! - Organization/Tenant CAs (intermediate CAs)
//! - Node certificates (end entities)
//!
//! ## Key Algorithm
//!
//! This utility generates Ed25519 certificates exclusively. Ed25519 is chosen for:
//! - Compatibility with libp2p's identity system
//! - Strong security with small key sizes
//! - Fast signature generation and verification
//! - Deterministic signatures
//!
//! The generated private keys are stored in PKCS#8 format, which is required
//! by the Hypha network's libp2p integration.
//!
//! ## Quick Start for Development
//!
//! ```bash
//! # 1. Generate Root CA
//! hypha-certutil root
//!
//! # 2. Generate Organization CA for tenant "acme-corp"
//! hypha-certutil org --root-cert hypha-space-root-ca-cert.pem \
//!                    --root-key hypha-space-root-ca-key.pem \
//!                    -o acme-corp
//!
//! # 3. Generate Node certificate
//! hypha-certutil node --ca-cert acme-corp-ca-cert.pem \
//!                     --ca-key acme-corp-ca-key.pem \
//!                     -n node1.acme-corp.hypha.network
//! ```
//!
//! ## Example Setup for Multi-Tenant Testing
//!
//! ```bash
//! # Create directory structure
//! mkdir -p certs/{root,tenants/{acme,globex}}
//!
//! # Generate root CA
//! hypha-certutil root -d certs/root
//!
//! # Generate CAs for two tenants
//! hypha-certutil org --root-cert certs/root/hypha-space-root-ca-cert.pem \
//!                    --root-key certs/root/hypha-space-root-ca-key.pem \
//!                    -o acme-corp -d certs/tenants/acme
//!
//! hypha-certutil org --root-cert certs/root/hypha-space-root-ca-cert.pem \
//!                    --root-key certs/root/hypha-space-root-ca-key.pem \
//!                    -o globex-corp -d certs/tenants/globex
//!
//! # Generate node certificates with custom SANs
//! hypha-certutil node --ca-cert certs/tenants/acme/acme-corp-ca-cert.pem \
//!                     --ca-key certs/tenants/acme/acme-corp-ca-key.pem \
//!                     -n api.acme.local \
//!                     -s api.acme.local,*.acme.local,10.0.0.1 \
//!                     -d certs/tenants/acme
//! ```
//!
use std::{
    fs, io,
    path::{Path, PathBuf},
};

use clap::{Parser, Subcommand};
use rcgen::{
    BasicConstraints, Certificate, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose,
};
use thiserror::Error;
use time::{Duration, OffsetDateTime};

#[derive(Error, Debug)]
pub enum CertError {
    #[error("Failed to generate certificate: {0}")]
    Rcgen(#[from] rcgen::Error),
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    #[error("Failed to load certificate: {0}")]
    Load(String),
}

#[derive(Parser)]
#[command(name = "hypha-certutil")]
#[command(about = "Certificate utility for Hypha network", long_about = None)]
#[command(version)]
#[command(after_help = "For more information and examples, see the module documentation")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
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
    /// The certificate will include a chain file for easy deployment.
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
        #[arg(
            short,
            long,
            value_delimiter = ',',
            default_value = "localhost,127.0.0.1,0.0.0.0"
        )]
        san: Vec<String>,

        /// Directory to save the certificate and key files
        #[arg(short, long, default_value = ".")]
        dir: PathBuf,
    },
}

/// Generate a Root CA certificate
fn generate_root_ca_certificate(
    common_name: &str,
    organization: &str,
) -> Result<Certificate, CertError> {
    let mut params = CertificateParams::new(vec![]);
    params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    params.distinguished_name.push(DnType::CountryName, "US");
    params
        .distinguished_name
        .push(DnType::OrganizationName, organization);
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);

    // Key usage for Root CA
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];

    // Set validity period (10 years for root CA)
    let now = OffsetDateTime::now_utc();
    // Valid from yesterday to avoid clock skew
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(3650);

    // Set algorithm to Ed25519
    params.alg = &rcgen::PKCS_ED25519;

    let key_pair = KeyPair::generate(&rcgen::PKCS_ED25519)?;
    params.key_pair = Some(key_pair);

    Certificate::from_params(params).map_err(|e| e.into())
}

/// Generate an Organization CA certificate signed by Root CA
fn generate_org_certificate(
    organization_name: &str,
    common_name: &str,
) -> Result<Certificate, CertError> {
    let mut params = CertificateParams::new(vec![]);
    params.is_ca = IsCa::Ca(BasicConstraints::Constrained(0)); // pathlen:0
    params.distinguished_name.push(DnType::CountryName, "US");
    params
        .distinguished_name
        .push(DnType::OrganizationName, organization_name);
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);

    // Key usage for Intermediate CA
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
    ];

    // Set validity period (5 years for org CAs)
    let now = OffsetDateTime::now_utc();

    // Valid from yesterday to avoid clock skew
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(1825);

    // Set algorithm to Ed25519
    params.alg = &rcgen::PKCS_ED25519;

    let key_pair = KeyPair::generate(&rcgen::PKCS_ED25519)?;
    params.key_pair = Some(key_pair);

    Certificate::from_params(params).map_err(|e| e.into())
}

/// Generate a certificate signed by a CA
fn generate_node_certificate(
    common_name: &str,
    san_names: Vec<String>,
) -> Result<Certificate, CertError> {
    let mut params = CertificateParams::new(san_names);
    params.distinguished_name.push(DnType::CountryName, "US");
    params
        .distinguished_name
        .push(DnType::OrganizationName, "Hypha Network");
    params
        .distinguished_name
        .push(DnType::CommonName, common_name);

    // Key usage for end-entity certificates
    params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyAgreement,
    ];

    params.extended_key_usages = vec![
        ExtendedKeyUsagePurpose::ServerAuth,
        ExtendedKeyUsagePurpose::ClientAuth,
    ];

    // Set validity period (1 year for node certificates)
    let now = OffsetDateTime::now_utc();
    // Valid from yesterday to avoid clock skew
    params.not_before = now - Duration::days(1);
    params.not_after = now + Duration::days(365);

    // Set algorithm to Ed25519
    params.alg = &rcgen::PKCS_ED25519;

    let key_pair = KeyPair::generate(&rcgen::PKCS_ED25519)?;
    params.key_pair = Some(key_pair);

    Certificate::from_params(params).map_err(|e| e.into())
}

/// Load a certificate and key pair from PEM files
fn load_ca_certificate(cert_path: &Path, key_path: &Path) -> Result<Certificate, CertError> {
    let cert_pem = fs::read_to_string(cert_path)
        .map_err(|e| CertError::Load(format!("Failed to read certificate file: {e}")))?;

    let key_pem = fs::read_to_string(key_path)
        .map_err(|e| CertError::Load(format!("Failed to read key file: {e}")))?;

    let params = CertificateParams::from_ca_cert_pem(&cert_pem, KeyPair::from_pem(&key_pem)?)
        .map_err(|e| CertError::Load(format!("Failed to parse certificate: {e}")))?;

    Certificate::from_params(params).map_err(|e| e.into())
}

fn main() -> Result<(), CertError> {
    let cli = Cli::parse();

    // Initialize crypto provider
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();

    match cli.command {
        Commands::Root {
            name,
            organization,
            dir,
        } => {
            // Use provided name or default to "<org> CA"
            let common_name = name.unwrap_or_else(|| format!("{organization} CA"));

            println!("Generating Root CA certificate...");
            let cert = generate_root_ca_certificate(&common_name, &organization)?;

            // Turn the (common) name into a valid file name for the certificate and key
            let file_name = common_name
                .to_lowercase()
                .replace(" ", "-")
                .replace("/", "-");

            let cert_out = dir.join(format!("{file_name}-cert.pem"));
            let key_out = dir.join(format!("{file_name}-key.pem"));

            fs::write(&cert_out, cert.serialize_pem()?)?;
            fs::write(&key_out, cert.serialize_private_key_pem())?;

            println!("Root CA certificate saved to: {}", cert_out.display());
            println!("Root CA private key saved to: {}", key_out.display());
            println!("\nNext steps:");
            println!("  Generate an Organization CA:");
            println!(
                "    hypha-certutil org --root-cert {} --root-key {} -o <org-name>",
                cert_out.display(),
                key_out.display()
            );
        }
        Commands::Org {
            root_cert,
            root_key,
            name,
            organization,
            dir,
        } => {
            println!("Generating Organization CA certificate...");

            // Load root CA certificate and key
            let root_ca = load_ca_certificate(&root_cert, &root_key)?;

            // Use provided name or default to "<org> CA"
            let common_name = name.unwrap_or_else(|| format!("{organization} CA"));

            // Generate org certificate
            let org_cert = generate_org_certificate(&organization, &common_name)?;

            // Sign the certificate with the root CA and get PEM directly
            let signed_cert_pem = org_cert.serialize_pem_with_signer(&root_ca)?;

            // Create filename from organization name
            let file_name = organization
                .to_lowercase()
                .replace(" ", "-")
                .replace("/", "-");

            let cert_out = dir.join(format!("{file_name}-ca-cert.pem"));
            let key_out = dir.join(format!("{file_name}-ca-key.pem"));

            // Write the signed certificate and private key
            fs::write(&cert_out, signed_cert_pem)?;
            fs::write(&key_out, org_cert.serialize_private_key_pem())?;

            println!(
                "Organization CA certificate saved to: {}",
                cert_out.display()
            );
            println!(
                "Organization CA private key saved to: {}",
                key_out.display()
            );
            println!("\nNext steps:");
            println!("  Generate node certificates:");
            println!(
                "    hypha-certutil node --ca-cert {} --ca-key {} -n <node-name>",
                cert_out.display(),
                key_out.display()
            );
        }
        Commands::Node {
            ca_cert,
            ca_key,
            name,
            san,
            dir,
        } => {
            println!("Generating node certificate...");

            // Load CA certificate and key
            let ca = load_ca_certificate(&ca_cert, &ca_key)?;

            // Generate node certificate
            let mut san_names = san;
            // Add the common name to SANs if not already present
            if !san_names.contains(&name) {
                san_names.push(name.clone());
            }

            let node_cert = generate_node_certificate(&name, san_names.clone())?;

            // Sign the certificate with the CA and get PEM directly
            let signed_cert_pem = node_cert.serialize_pem_with_signer(&ca)?;

            // Create filename from common name
            let file_name = name
                .to_lowercase()
                .replace(" ", "-")
                .replace("/", "-")
                .replace(".", "-");

            let cert_out = dir.join(format!("{file_name}-cert.pem"));
            let key_out = dir.join(format!("{file_name}-key.pem"));
            let chain_out = dir.join(format!("{file_name}-chain.pem"));

            // Load CA certificate PEM for chain creation
            let ca_cert_pem = fs::read_to_string(&ca_cert)?;

            // Write individual certificate and key
            fs::write(&cert_out, &signed_cert_pem)?;
            fs::write(&key_out, node_cert.serialize_private_key_pem())?;

            // Create certificate chain (node cert + CA cert)
            let chain_pem = format!("{signed_cert_pem}{ca_cert_pem}");
            fs::write(&chain_out, chain_pem)?;

            println!("Node certificate saved to: {}", cert_out.display());
            println!("Node private key saved to: {}", key_out.display());
            println!("Certificate chain saved to: {}", chain_out.display());
            println!("\nCertificate details:");
            println!("  Common Name: {name}");
            println!("  SANs: {}", san_names.join(", "));

            // If we can find a root CA cert, create full chain
            if let Some(parent) = ca_cert.parent() {
                let root_cert_path = parent.join("hypha-space-root-ca-cert.pem");
                if root_cert_path.exists() {
                    let root_cert_pem = fs::read_to_string(&root_cert_path)?;
                    let fullchain_out = dir.join(format!("{file_name}-fullchain.pem"));
                    let fullchain_pem = format!("{signed_cert_pem}{ca_cert_pem}{root_cert_pem}");
                    fs::write(&fullchain_out, fullchain_pem)?;
                    println!(
                        "Full certificate chain saved to: {}",
                        fullchain_out.display()
                    );
                }
            }
        }
    }

    Ok(())
}
