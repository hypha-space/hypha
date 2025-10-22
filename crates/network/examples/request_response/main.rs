use std::{collections::HashMap, error::Error, fs, path::PathBuf, time::Duration};

use clap::{Parser, Subcommand};
use futures_util::StreamExt;
use hypha_network::{
    IpNet,
    cert::{load_certs_from_pem, load_crls_from_pem, load_private_key_from_pem},
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterfaceExt,
    },
    swarm::{SwarmDriver, SwarmError},
};
use libp2p::{
    Multiaddr, Swarm, SwarmBuilder,
    request_response::{self, ProtocolSupport, cbor::codec::Codec},
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[clap(
    name = "request-response",
    bin_name = "cargo run --example request_response",
    about = "Example of a request-response protocol",
    version
)]
struct Args {
    #[clap(subcommand)]
    variant: Variants,
    #[clap(long)]
    cert_file: PathBuf,
    #[clap(long)]
    key_file: PathBuf,
    #[clap(long = "trust-file")]
    trust_file: PathBuf,
    #[clap(long)]
    crl_file: Option<PathBuf>,
}

#[derive(Subcommand, Clone)]
enum Variants {
    Server {
        #[clap(long, default_value = "/ip4/0.0.0.0/tcp/0")]
        listen_addr: String,
    },
    Client {
        #[clap(long)]
        server_addr: String,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env())
        .init();

    let args = Args::parse();

    match args.variant.clone() {
        Variants::Server { listen_addr } => server(args, &listen_addr).await?,
        Variants::Client { server_addr } => client(args, &server_addr).await?,
    }

    Ok(())
}

async fn server(args: Args, listen_addr: &str) -> Result<(), Box<dyn Error>> {
    let (network, driver) = create_network(args).await?;
    tokio::spawn(driver.run());

    let listen_addr = listen_addr.parse::<Multiaddr>()?;
    network.listen(listen_addr).await?;
    tracing::info!("Server listening and ready to handle requests");

    let handler = network
        .on(|req: &ExampleRequest| matches!(req, ExampleRequest::Ping(_) | ExampleRequest::Echo(_)))
        .into_stream()
        .await?;

    tokio::spawn(async move {
        handler
            .respond_with_concurrent(Some(10), |(_, req)| async move {
                match req {
                    ExampleRequest::Ping(msg) => {
                        tracing::info!("Received ping: {}", msg);
                        ExampleResponse::Pong(format!("Pong: {msg}"))
                    }
                    ExampleRequest::Echo(msg) => {
                        tracing::info!("Received echo: {}", msg);
                        ExampleResponse::Echo(msg)
                    }
                }
            })
            .await;
    });

    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutting down server");
    Ok(())
}

async fn client(args: Args, server_addr: &str) -> Result<(), Box<dyn Error>> {
    let (network, driver) = create_network(args).await?;
    tokio::spawn(driver.run());

    let server_addr = server_addr.parse::<Multiaddr>()?;
    let peer_id = network.dial(server_addr).await?;
    tracing::info!("Connected to server: {}", peer_id);

    tokio::time::sleep(Duration::from_millis(500)).await;

    let ping_response = network
        .request(
            peer_id,
            ExampleRequest::Ping("Hello from client".to_string()),
        )
        .await?;
    tracing::info!("Ping response: {:?}", ping_response);

    let echo_response = network
        .request(peer_id, ExampleRequest::Echo("Echo test".to_string()))
        .await?;
    tracing::info!("Echo response: {:?}", echo_response);

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum ExampleRequest {
    Ping(String),
    Echo(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
enum ExampleResponse {
    Pong(String),
    Echo(String),
}

type ExampleCodec = Codec<ExampleRequest, ExampleResponse>;

async fn create_network(args: Args) -> Result<(Network, NetworkDriver), Box<dyn Error>> {
    // Load certificates and private key
    let cert_pem = fs::read(&args.cert_file)?;
    let key_pem = fs::read(&args.key_file)?;
    let ca_cert_pem = fs::read(&args.trust_file)?;

    let cert_chain = load_certs_from_pem(&cert_pem)?;
    let private_key = load_private_key_from_pem(&key_pem)?;
    let ca_certs = load_certs_from_pem(&ca_cert_pem)?;

    // Optionally load CRLs to revoke access
    let crls = if let Some(crl_file) = args.crl_file {
        let crl_pem = fs::read(&crl_file)?;
        load_crls_from_pem(&crl_pem)?
    } else {
        vec![]
    };

    let swarm = SwarmBuilder::with_existing_identity(cert_chain, private_key, ca_certs, crls)
        .with_tokio()
        .with_tcp(
            tcp::Config::default(),
            tls::Config::new,
            yamux::Config::default,
        )
        .map_err(|_| "Failed to create TCP transport.")?
        .with_behaviour(|_key| Behaviour {
            request_response: request_response::Behaviour::<ExampleCodec>::new(
                [(
                    libp2p::StreamProtocol::new("/example/0.0.1"),
                    ProtocolSupport::Full,
                )],
                request_response::Config::default(),
            ),
        })
        .map_err(|_| "Failed to create swarm behavior.")?
        .build();

    let (network, driver) = Network::create(swarm);
    Ok((network, driver))
}

#[derive(NetworkBehaviour)]
struct Behaviour {
    request_response: request_response::Behaviour<ExampleCodec>,
}

impl RequestResponseBehaviour<ExampleCodec> for Behaviour {
    fn request_response(&mut self) -> &mut request_response::Behaviour<ExampleCodec> {
        &mut self.request_response
    }
}

enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    RequestResponse(RequestResponseAction<ExampleCodec>),
}

struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials: PendingDials,
    pending_listens: PendingListens,
    outbound_requests: OutboundRequests<ExampleCodec>,
    outbound_responses: OutboundResponses,
    request_handlers: Vec<RequestHandler<ExampleCodec>>,
    action_rx: mpsc::Receiver<Action>,
}

impl SwarmDriver<Behaviour> for NetworkDriver {
    fn swarm(&mut self) -> &mut Swarm<Behaviour> {
        &mut self.swarm
    }

    async fn run(mut self) -> Result<(), SwarmError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(event)) => {
                            self.process_request_response_event(event).await;
                        }
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => {
                            tracing::info!("Connected to {}", peer_id);
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { listener_id, address, .. } => {
                            self.process_new_listen_addr(&listener_id, address).await;
                        }
                        _ => {}
                    }
                }
                Some(action) = self.action_rx.recv() => {
                    match action {
                        Action::Dial(action) => self.process_dial_action(action).await,
                        Action::Listen(action) => self.process_listen_action(action).await,
                        Action::RequestResponse(action) => self.process_request_response_action(action).await,
                    }
                }
                else => break
            }
        }

        Ok(())
    }
}

impl RequestResponseDriver<Behaviour, ExampleCodec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<ExampleCodec> {
        &mut self.outbound_requests
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses
    }

    fn request_handlers(&mut self) -> &mut Vec<RequestHandler<ExampleCodec>> {
        &mut self.request_handlers
    }
}

impl DialDriver<Behaviour> for NetworkDriver {
    fn pending_dials(&mut self) -> &mut PendingDials {
        &mut self.pending_dials
    }

    fn exclude_cidrs(&self) -> &[IpNet] {
        &[]
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(&mut self) -> &mut PendingListens {
        &mut self.pending_listens
    }
}

#[derive(Clone)]
struct Network {
    action_tx: mpsc::Sender<Action>,
}

impl Network {
    fn create(swarm: Swarm<Behaviour>) -> (Self, NetworkDriver) {
        let (action_tx, action_rx) = mpsc::channel(100);

        let driver = NetworkDriver {
            swarm,
            pending_dials: HashMap::new(),
            pending_listens: HashMap::new(),
            outbound_requests: HashMap::new(),
            outbound_responses: HashMap::new(),
            request_handlers: Vec::new(),
            action_rx,
        };

        let interface = Self { action_tx };

        (interface, driver)
    }
}

impl hypha_network::request_response::RequestResponseInterface<ExampleCodec> for Network {
    async fn send(&self, action: RequestResponseAction<ExampleCodec>) {
        let _ = self.action_tx.send(Action::RequestResponse(action)).await;
    }

    fn try_send(
        &self,
        action: RequestResponseAction<ExampleCodec>,
    ) -> Result<(), RequestResponseError> {
        self.action_tx
            .try_send(Action::RequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Driver dropped".to_string()))
    }
}

impl DialInterface for Network {
    async fn send(&self, action: DialAction) {
        let _ = self.action_tx.send(Action::Dial(action)).await;
    }
}

impl ListenInterface for Network {
    async fn send(&self, action: ListenAction) {
        let _ = self.action_tx.send(Action::Listen(action)).await;
    }
}

#[cfg(test)]
mod tests {
    use libp2p_swarm_test::SwarmExt;

    use super::*;

    fn create_test_swarm() -> Swarm<Behaviour> {
        Swarm::new_ephemeral_tokio(|_| Behaviour {
            request_response: request_response::Behaviour::new(
                [(
                    libp2p::StreamProtocol::new("/test/0.0.1"),
                    ProtocolSupport::Full,
                )],
                request_response::Config::default(),
            ),
        })
    }

    #[tokio::test]
    async fn test_protocol() {
        let mut swarm1 = create_test_swarm();
        let mut swarm2 = create_test_swarm();

        swarm1.listen().with_memory_addr_external().await;
        swarm2.listen().with_memory_addr_external().await;

        swarm1.connect(&mut swarm2).await;

        let peer2 = *swarm2.local_peer_id();

        let (interface1, driver1) = Network::create(swarm1);
        let (interface2, driver2) = Network::create(swarm2);

        tokio::spawn(driver1.run());
        tokio::spawn(driver2.run());

        let handler = interface2
            .on(|req: &ExampleRequest| matches!(req, ExampleRequest::Ping(_)))
            .into_stream()
            .await
            .unwrap();

        tokio::spawn(async move {
            handler
                .respond_with_concurrent(Some(1), |req| async move {
                    match req {
                        ExampleRequest::Ping(msg) => ExampleResponse::Pong(format!("Got: {}", msg)),
                        _ => unreachable!(),
                    }
                })
                .await;
        });

        let response = interface1
            .request(peer2, ExampleRequest::Ping("Hello".to_string()))
            .await
            .unwrap();

        assert_eq!(response, ExampleResponse::Pong("Got: Hello".to_string()));
    }
}
