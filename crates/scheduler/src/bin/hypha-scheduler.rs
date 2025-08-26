//! Scheduler binary.

use std::{fs, time::Duration};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use futures_util::future::join_all;
use hypha_config::{ConfigWithMetadata, ConfigWithMetadataTLSExt, builder, to_toml};
use hypha_messages::{
    Executor, Fetch, JobSpec, Receive, Requirement, Resources, SelectionStrategy, Send, WorkerSpec,
};
use hypha_network::{
    dial::DialInterface, kad::KademliaInterface, listen::ListenInterface, swarm::SwarmDriver,
};
use hypha_scheduler::{
    allocator::{Allocator, GreedyWorkerAllocator},
    config::Config,
    network::Network,
};
use libp2p::{multiaddr::Protocol, Multiaddr};
use miette::{IntoDiagnostic, Result};
use serde::Serialize;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser, Serialize)]
#[command(
    name = "hypha-scheduler",
    version,
    about = "Hypha Scheduler",
    long_about = "Runs the Hypha Scheduler coordinating workers.",
    after_help = "For more information, see the project documentation."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand, Serialize)]
enum Commands {
    Init {
        /// Path where the configuration file will be written
        #[clap(short, long, default_value = "config.toml")]
        output: std::path::PathBuf,
    },
    #[serde(untagged)]
    Run {
        /// Path to the configuration file.
        #[clap(short, long("config"), default_value = "config.toml")]
        #[serde(skip)]
        config_file: std::path::PathBuf,

        /// Addresses of the gateways (can be specified multiple times).
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_addresses: Option<Vec<Multiaddr>>,

        /// Addresses to listen on (can be specified multiple times).
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_addresses: Option<Vec<Multiaddr>>,
    },
}

async fn run(config: ConfigWithMetadata<Config>) -> Result<()> {
    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let (network, network_driver) =
        Network::create(cert_chain, private_key, ca_certs, crls).into_diagnostic()?;
    tokio::spawn(network_driver.run());

    join_all(
        config
            .listen_addresses()
            .iter()
            .map(|address| network.listen(address.clone()))
            .collect::<Vec<_>>(),
    )
    .await
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .into_diagnostic()?;
    tracing::info!("Successfully listening on all addresses");

    // Dial each gateway and, on success, set up a relay circuit listen via it.
    let gateway_results = join_all(
        config
            .gateway_addresses()
            .iter()
            .map(|address| {
                let address = address.clone();
                let network = network.clone();
                async move {
                    match network.dial(address.clone()).await {
                        Ok(peer_id) => {
                            // Attempt to listen on the relay circuit via the connected gateway.
                            match address
                                .with_p2p(peer_id)
                                .map(|a| a.with(Protocol::P2pCircuit))
                            {
                                Ok(relay_addr) => {
                                    if let Err(e) = network.listen(relay_addr).await {
                                        tracing::warn!(error=%e, "Failed to set up P2pCircuit listen via gateway");
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!(error=%e, "Failed to construct relay listen address");
                                }
                            }

                            Ok(peer_id)
                        }
                        Err(e) => Err(e),
                    }
                }
            })
            .collect::<Vec<_>>(),
    )
    .await;

    let gateway_peer_ids: Vec<_> = gateway_results
        .into_iter()
        .filter_map(|result| result.ok())
        .collect();

    if gateway_peer_ids.is_empty() {
        return Err(miette::miette!("Failed to connect to any gateway"));
    }

    tracing::info!(gateway_ids = ?gateway_peer_ids, "Connected to gateway(s)");

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await.into_diagnostic()?;

    // NOTE: Create allocator for resource allocation alongside existing job management
    let allocator = GreedyWorkerAllocator::new(network.clone());

    tracing::info!("Starting worker allocation and job creation process");

    // NOTE: Phase 1 - Allocate workers using the new allocator/arbiter protocol
    // First, request worker nodes for job execution
    let worker_spec = WorkerSpec {
        requirements: vec![
            Requirement::Resource(Resources::Gpu { min: 10.0 }),
            Requirement::Resource(Resources::Cpu { min: 1.0 }),
            Requirement::Resource(Resources::Memory { min: 1.0 }),
            Requirement::Driver {
                kind: "diloco-transformer".into(),
            },
        ],
    };

    // Request multiple workers to increase chances of two distinct peers
    let mut allocated_workers = Vec::new();
    for i in 0..2 {
        tracing::info!(worker_index = i, "Requesting worker allocation");
        match allocator.request(worker_spec.clone(), 100.0, None).await {
            Ok(worker) => {
                tracing::info!(
                    peer_id = %worker.peer_id(),
                    lease_id = %worker.lease_id(),
                    "Successfully allocated worker"
                );
                allocated_workers.push(worker);
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to allocate worker");
            }
        }
        sleep(Duration::from_millis(100)).await;
    }

    let parameter_server_spec = WorkerSpec {
        requirements: vec![
            Requirement::Resource(Resources::Cpu { min: 1.0 }),
            Requirement::Resource(Resources::Memory { min: 1.0 }),
            Requirement::Driver {
                kind: "parameter-server".into(),
            },
        ],
    };

    // Request multiple workers to increase chances of two distinct peers
    let mut allocated_parameter_servers = Vec::new();
    for i in 0..1 {
        tracing::info!(worker_index = i, "Requesting worker allocation");
        match allocator
            .request(parameter_server_spec.clone(), 100.0, None)
            .await
        {
            Ok(worker) => {
                tracing::info!(
                    peer_id = %worker.peer_id(),
                    lease_id = %worker.lease_id(),
                    "Successfully allocated worker"
                );
                allocated_parameter_servers.push(worker);
            }
            Err(e) => {
                tracing::error!(error = %e, "Failed to allocate worker");
            }
        }
        sleep(Duration::from_millis(100)).await;
    }

    if allocated_workers.len() > 1 && allocated_parameter_servers.len() == 1 {
        let all_peers = allocated_parameter_servers
            .iter()
            .map(|ps| ps.peer_id())
            .collect::<Vec<_>>();
        let parameter_server = &mut allocated_parameter_servers[0];

        for worker in &mut allocated_workers {
            worker
                .dispatch(JobSpec {
                    executor: Executor::DiLoCoTransformer {
                        model: Fetch::uri("EleutherAI/gpt-neo-125m"), // Fetch::uri("https://example.com/"),
                        data: Fetch::uri("datablations/c4-filter-small"),
                        updates: Receive::peers(vec![parameter_server.peer_id()]),
                        results: Send::peers(
                            vec![parameter_server.peer_id()],
                            SelectionStrategy::One,
                        ),
                    },
                })
                .await
                .into_diagnostic()?;
        }
        let _p_rx = parameter_server
            .dispatch(JobSpec {
                executor: Executor::ParameterServer {
                    updates: Receive::peers(
                        allocated_workers
                            .iter()
                            .map(|worker| worker.peer_id())
                            .collect(),
                    ),
                    results: Send::peers(all_peers, SelectionStrategy::All),
                },
            })
            .await
            .into_diagnostic()?;
    } else {
        tracing::warn!("Insufficient workers allocated for diloco training");
    }

    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &to_toml(&Config::default())?).into_diagnostic()?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config: ConfigWithMetadata<Config> = builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_SCHEDULER_"))
                .with_provider(Serialized::defaults(args))
                .build()?;

            return run(config).await;
        }
    }
}
