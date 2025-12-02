use std::{fs, time::Duration};

use clap::Parser;
use console::style;
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_inspect::{config::Config, network::Network};
use hypha_messages::health;
use hypha_network::{
    cert, dial::DialInterface, kad::KademliaInterface,
    request_response::RequestResponseInterfaceExt, swarm::SwarmDriver,
};
use indicatif::{ProgressBar, ProgressDrawTarget, ProgressStyle};
use libp2p::Multiaddr;
use miette::{IntoDiagnostic, Result};
use tracing_indicatif::{
    IndicatifLayer,
    filter::{IndicatifFilter, hide_indicatif_span_fields},
};
use tracing_subscriber::{
    EnvFilter, Layer, fmt::format::DefaultFields, layer::SubscriberExt, util::SubscriberInitExt,
};

#[path = "../cli.rs"]
mod cli;
use cli::{Cli, Commands};

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Initialize tracing (simplified for CLI tool)
    // Only initialize if NO OTHER subscriber is set (e.g. via env vars)
    let indicatif_layer = IndicatifLayer::new()
        .with_span_field_formatter(hide_indicatif_span_fields(DefaultFields::new()));
    let indicatif_writer = indicatif_layer.get_stderr_writer();
    let indicatif_layer = indicatif_layer.with_filter(IndicatifFilter::new(false));
    if tracing_subscriber::registry()
        .with(indicatif_layer)
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(indicatif_writer)
                .with_filter(EnvFilter::from_default_env()),
        )
        .try_init()
        .is_err()
    {
        // Already initialized? Ignore.
    }

    match &cli.command {
        args @ Commands::Probe {
            config_file,
            address,
            timeout,
            ..
        } => {
            let spinner = create_spinner();

            spinner.set_message(format!("Dailing {address}..."));

            let config = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(args))
                .build()?
                .validate()?;

            let cert_chain = config.load_cert_chain()?;
            let private_key = config.load_key()?;
            let ca_certs = config.load_trust_chain()?;
            let crls = config.load_crls()?;

            let exclude_cidrs = hypha_network::reserved_cidrs();

            let (network, network_driver) =
                Network::create(cert_chain, private_key, ca_certs, crls, exclude_cidrs)
                    .into_diagnostic()?;

            let network_driver_task = tokio::spawn(network_driver.run());

            let addr: Multiaddr = address.parse().into_diagnostic()?;

            tokio::time::timeout(Duration::from_millis(*timeout), {
                let network = network.clone();

                async move {
                    let peer_id = network.dial(addr).await.into_diagnostic()?;

                    spinner.set_message(format!("Health checking {peer_id}..."));

                    let resp = network
                        .request::<health::Codec>(peer_id, health::Request {})
                        .await
                        .into_diagnostic()?;

                    spinner.finish_and_clear();

                    println!(
                        "{} {}",
                        style("Address").bold(),
                        style(format!("({address})")).dim()
                    );
                    println!("├─ {} {}", style("Peer ID:").bold(), style(peer_id));

                    if resp.healthy {
                        println!(
                            "├─ {} {}",
                            style("Status:").bold(),
                            style("Healthy").green()
                        );

                        Ok(())
                    } else {
                        println!(
                            "├─ {} {}",
                            style("Status:").bold(),
                            style("Unhealthy").red()
                        );

                        Err(miette::miette!("The peer is unhealthy"))
                    }
                }
            })
            .await
            .into_diagnostic()??;

            drop(network);
            if !network_driver_task.is_finished() {
                network_driver_task.abort();
            }
            Ok(())
        }
        args @ Commands::Lookup {
            config_file,
            peer_id,
            ..
        } => {
            let peer_id = *peer_id;
            let spinner = create_spinner();

            spinner.set_message("Joining network...");

            let config = builder::<Config>()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_"))
                .with_provider(Serialized::defaults(args))
                .build()?
                .validate()?;

            let cert_chain = config.load_cert_chain()?;
            let private_key = config.load_key()?;
            let ca_certs = config.load_trust_chain()?;
            let crls = config.load_crls()?;

            let exclude_cidrs = hypha_network::reserved_cidrs();

            let (network, network_driver) =
                Network::create(cert_chain, private_key, ca_certs, crls, exclude_cidrs)
                    .into_diagnostic()?;

            let network_driver_task = tokio::spawn(network_driver.run());

            let gateway_peer_ids: Vec<_> = join_all(
                config
                    .gateway_addresses()
                    .iter()
                    .map(|address| {
                        let network = network.clone();
                        async move { network.dial(address.clone()).await }
                    })
                    .collect::<Vec<_>>(),
            )
            .await
            .into_iter()
            .filter_map(|result| result.ok())
            .collect();

            if gateway_peer_ids.is_empty() {
                return Err(miette::miette!("Failed to connect to any gateway"));
            }

            tracing::info!(gateway_ids = ?gateway_peer_ids, "Connected to gateway(s)");

            network.wait_for_bootstrap().await.into_diagnostic()?;

            spinner.set_message(format!("Querying DHT for {peer_id}..."));

            let closest_peers = network.get_closest_peers(peer_id).await.into_diagnostic()?;
            if let Some(peer) = closest_peers
                .iter()
                .find(|peer| peer.peer_id == peer_id)
                .cloned()
            {
                spinner.finish_and_clear();

                println!(
                    "{} {}",
                    style("Peer").bold(),
                    style(format!("({peer_id})")).dim()
                );
                for addr in peer.addrs {
                    println!("├─ {} {}", style("Address:").dim(), style(addr));
                }
            } else {
                return Err(miette::miette!("No peer with ID {} found.", peer_id));
            }

            drop(network);
            if !network_driver_task.is_finished() {
                network_driver_task.abort();
            }
            Ok(())
        }
        Commands::CertInfo { path } => {
            let path = path.clone();
            let spinner = create_spinner();
            spinner.set_message(format!("Inspecting {}...", path.display()));
            let content = fs::read(&path).into_diagnostic()?;

            // Try private key first
            if let Ok(key) = cert::load_private_key_from_pem(&content)
                && let Ok(identity) = cert::identity_from_private_key(&key)
            {
                spinner.finish_and_clear();

                let peer_id = identity.public().to_peer_id();
                println!(
                    "{} {}",
                    style("Private Key").bold().on_red(),
                    style(format!("({})", path.display())).dim()
                );
                println!("├─ {} {}", style("Peer ID:").bold(), style(peer_id).cyan());

                return Ok(());
            }

            // Try certificate
            let (_rem, pem) = x509_parser::pem::parse_x509_pem(&content).into_diagnostic()?;
            let cert = pem.parse_x509().into_diagnostic()?;

            let ed_key = libp2p::identity::ed25519::PublicKey::try_from_bytes(
                &cert.tbs_certificate.subject_pki.subject_public_key.data,
            )
            .into_diagnostic()?;

            let peer_id = libp2p::identity::PublicKey::from(ed_key).to_peer_id();
            spinner.finish_and_clear();

            println!(
                "{} {}",
                style("Certificate").bold(),
                style(format!("({})", path.display())).dim()
            );
            println!("├─ {} {}", style("Peer ID:").bold(), style(peer_id).cyan());

            Ok(())
        }
        args @ Commands::Init { output, .. } => {
            let output = output.clone();
            let config = builder::<Config>()
                .with_provider(Serialized::defaults(&Config::default()))
                .with_provider(Serialized::defaults(args))
                .build()?
                .validate()?;

            fs::write(&output, &to_toml(&config.config).into_diagnostic()?).into_diagnostic()?;

            println!("Configuration written to: {:?}", output);
            Ok(())
        }
    }
}

fn create_spinner() -> ProgressBar {
    let spinner = ProgressBar::new_spinner();
    spinner.set_draw_target(ProgressDrawTarget::stderr_with_hz(60));
    spinner.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .expect("valid spinner template")
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
    );
    spinner.enable_steady_tick(Duration::from_millis(120));

    spinner
}
