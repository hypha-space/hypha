//! Scheduler binary.

use std::{error::Error, fs, time::Duration};

use clap::{Parser, Subcommand};
use figment::providers::{Env, Format, Serialized, Toml};
use hypha_config::LayeredConfig;
use hypha_messages::{
    Executor, Fetch, JobSpec, Receive, Requirement, Resources, SelectionStrategy, Send, WorkerSpec,
};
use hypha_network::{
    dial::DialInterface,
    kad::KademliaInterface,
    listen::ListenInterface,
    swarm::SwarmDriver,
    utils::{multiaddr_from_socketaddr, multiaddr_from_socketaddr_quic},
};
use hypha_scheduler::{
    allocator::{Allocator, GreedyWorkerAllocator},
    config::Config,
    network::Network,
};
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

        /// Address of the gateway.
        #[clap(long("gateway"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        gateway_address: Option<std::net::SocketAddr>,

        /// Address to listen on.
        #[clap(long("listen"))]
        #[serde(skip_serializing_if = "Option::is_none")]
        listen_address: Option<std::net::SocketAddr>,
    },
}

async fn run(config: Config) -> Result<(), Box<dyn Error>> {
    let cert_chain = config.load_cert_chain()?;
    let private_key = config.load_key()?;
    let ca_certs = config.load_trust_chain()?;
    let crls = config.load_crls()?;

    let gateway_address = multiaddr_from_socketaddr(config.gateway_address())?;

    let (network, network_driver) = Network::create(cert_chain, private_key, ca_certs, crls)?;
    tokio::spawn(network_driver.run());

    network
        .listen(multiaddr_from_socketaddr(config.listen_address())?)
        .await?;
    network
        .listen(multiaddr_from_socketaddr_quic(config.listen_address())?)
        .await?;

    tracing::info!("Successfully listening");

    // Dial the gateway address
    // TODO: fall back to TCP if QUIC doesn't work
    let _gateway_peer_id = network.dial(gateway_address).await?;

    // Wait until DHT bootstrapping is done.
    network.wait_for_bootstrap().await?;

    // NOTE: Create allocator for resource allocation alongside existing job management
    let allocator = GreedyWorkerAllocator::new(network.clone());

    // tokio::spawn(async move {
    //     if let Err(e) = allocator_driver.run().await {
    //         tracing::error!(error = %e, "Allocator driver failed");
    //     }
    // });

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

    if allocated_workers.len() >= 1 && allocated_parameter_servers.len() == 1 {
        let parameterserver = &allocated_parameter_servers[0];

        for worker in &allocated_workers {
            worker
                .dispatch(JobSpec {
                    executor: Executor::DiLoCoTransformer {
                        model: Fetch::uri("EleutherAI/gpt-neo-125m"), // Fetch::uri("https://example.com/"),
                        data: Fetch::uri("datablations/c4-filter-small"),
                        updates: Receive::peers(vec![parameterserver.peer_id()]),
                        results: Send::peers(
                            vec![parameterserver.peer_id()],
                            SelectionStrategy::One,
                        ),
                    },
                })
                .await?;
        }
        let _p_rx = parameterserver
            .dispatch(JobSpec {
                executor: Executor::ParameterServer {
                    updates: Receive::peers(
                        allocated_workers
                            .iter()
                            .map(|worker| worker.peer_id())
                            .collect(),
                    ),
                    results: Send::peers(
                        allocated_parameter_servers
                            .iter()
                            .map(|ps| ps.peer_id())
                            .collect(),
                        SelectionStrategy::All,
                    ),
                },
            })
            .await?;
    } else {
        tracing::warn!("Insufficient workers allocated for diloco training");
    }
    // for worker in allocated_workers {
    //     tracing::info!(
    //         peer_id = %worker.peer_id(),
    //         lease_id = %worker.lease_id(),
    //         "Successfully allocated worker"
    //     );

    // loop {
    //     match job_events.recv().await {
    //         Some(JobStatus::Running) => {
    //             tracing::info!("Running job");
    //         }
    //         Some(JobStatus::Failed) => {
    //             tracing::error!("Failed to allocate worker");
    //             break;
    //         }
    //         Some(JobStatus::Finished) => {
    //             tracing::info!("Completed job");
    //             break;
    //         }
    //         Some(JobStatus::Unknown) => {
    //             tracing::warn!("Unexpected job status");
    //             break;
    //         }
    //         None => {
    //             tracing::error!("Failed to complete job");
    //             break;
    //         }
    //     }
    // }

    // NOTE: Phase 2 - If we have sufficient workers, dispatch jobs
    // if allocated_workers.len() >= 2 && parameter_server.is_some() {
    //     tracing::info!(
    //         worker_count = allocated_workers.len(),
    //         "Starting job with allocated workers"
    //     );

    //     start_job(
    //         network.clone(),
    //         job_id,
    //         parameter_server_peer_id.unwrap(),
    //         allocated_workers,
    //     )
    //     .await;
    // } else {
    //     tracing::warn!(
    //         allocated_workers = allocated_workers.len(),
    //         has_parameter_server = parameter_server_peer_id.is_some(),
    //         "Insufficient workers allocated - falling back to legacy announcement"
    //     );

    //     // Fallback to legacy announcement for existing workers
    //     let mut data = Vec::new();
    //     ciborium::into_writer(
    //         &hypha_messages::RequestAnnounce {
    //             job_id,
    //             scheduler_peer_id: local_peer_id,
    //         },
    //         &mut data,
    //     )
    //     .unwrap();
    //     let _ = network.publish("announce", data).await;

    //     // NOTE: In legacy mode, we need to handle worker responses
    //     // Uncomment and enable the handler for legacy worker requests
    //     tracing::info!("Running in legacy mode, waiting for worker responses");
    //     // TODO: Re-enable legacy handler when needed
    //     tokio::time::sleep(Duration::from_secs(60)).await;
    // }

    let _ = tokio::signal::ctrl_c().await;
    Ok(())
}

// async fn start_job(
//     network: Network,
//     job_id: TaskId,
//     parameter_server_peer_id: PeerId,
//     worker_peer_ids: Vec<PeerId>,
// ) {
//     let mut set = JoinSet::new();

//     for worker_peer_id in worker_peer_ids {
//         tracing::info!(job_id = %job_id, peer_id = %worker_peer_id, "Requesting work");
//         let network = network.clone();
//         set.spawn(async move {
//             let result = network
//                 .request(
//                     worker_peer_id,
//                     hypha_messages::Request::Scheduler(hypha_messages::SchedulerRequest::Work {
//                         job_id,
//                         parameter_server_peer_id,
//                     }),
//                 )
//                 .await;

//             match &result {
//                 Ok(_) => {
//                     tracing::info!(
//                         job_id = %job_id,
//                         peer_id = %worker_peer_id,
//                         "Successfully sent work request"
//                     );
//                 }
//                 Err(e) => {
//                     tracing::error!(
//                         job_id = %job_id,
//                         peer_id = %worker_peer_id,
//                         error = %e,
//                         "Failed to send work request"
//                     );
//                 }
//             }
//             result
//         });
//     }

//     let results = set.join_all().await;
//     let successful_requests = results.iter().filter(|r| r.is_ok()).count();
//     tracing::info!(
//         job_id = %job_id,
//         successful_requests = successful_requests,
//         total_requests = results.len(),
//         "Task dispatch completed"
//     );
// }

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();
    match &cli.command {
        Commands::Init { output } => {
            fs::write(output, &Config::default().to_toml()?)?;

            println!("Configuration written to: {output:?}");
            Ok(())
        }
        args @ Commands::Run { config_file, .. } => {
            let config = Config::builder()
                .with_provider(Toml::file(config_file))
                .with_provider(Env::prefixed("HYPHA_SCHEDULER_"))
                .with_provider(Serialized::defaults(args))
                .build()
                .map_err(|e| format!("Configuration error: {e}"))?;

            return run(config).await;
        }
    }
}
