use std::{collections::HashMap, sync::Arc};

use futures_util::StreamExt;
use hypha_messages::{DataSlice, health};
use hypha_network::{
    CertificateDer, CertificateRevocationListDer, IpNet, PrivateKeyDer,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    kad::{KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, PendingQueries},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterface,
    },
    stream_pull::{StreamPullInterface, StreamPullReceiverInterface},
    swarm::{SwarmDriver, SwarmError},
};
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder, dcutr, gossipsub, identify, kad, ping, relay,
    request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use libp2p_stream as stream;
use tokio::sync::{SetOnce, mpsc};

type HealthRequestHandlers = Vec<RequestHandler<health::Codec>>;

#[derive(Clone)]
pub struct Network {
    action_sender: mpsc::Sender<Action>,
    stream_control: stream::Control,
}

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    health_request_response: request_response::Behaviour<health::Codec>,
}

pub struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    pending_bootstrap: Arc<SetOnce<()>>,
    action_receiver: mpsc::Receiver<Action>,
    health_outbound_requests_map: OutboundRequests<health::Codec>,
    health_outbound_responses_map: OutboundResponses,
    health_request_handlers: HealthRequestHandlers,
    exclude_cidrs: Vec<IpNet>,
}

#[allow(clippy::large_enum_variant)]
enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    HealthRequestResponse(RequestResponseAction<health::Codec>),
}

impl Network {
    pub fn create(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
        exclude_cidrs: Vec<IpNet>,
    ) -> Result<(Self, NetworkDriver), SwarmError> {
        let (action_sender, action_receiver) = mpsc::channel(5);

        let swarm = SwarmBuilder::with_existing_identity(cert_chain, private_key, ca_certs, crls)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                tls::Config::new,
                yamux::Config::default,
            )
            .map_err(|_| {
                SwarmError::TransportConfig("Failed to create TCP transport.".to_string())
            })?
            .with_quic()
            .with_relay_client(tls::Config::new, yamux::Config::default)
            .map_err(|_| {
                SwarmError::TransportConfig("Failed to create relay client with mTLS.".to_string())
            })?
            .with_behaviour(|key, relay_client| {
                let gossipsub = gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub::Config::default(),
                )
                .expect("Failed to create gossipsub behaviour");

                Behaviour {
                    ping: ping::Behaviour::new(ping::Config::new()),
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/hypha-identify/0.0.1".to_string(),
                        key.public(),
                    )),
                    relay_client,
                    dcutr: dcutr::Behaviour::new(key.public().to_peer_id()),
                    stream: stream::Behaviour::new(),
                    kademlia: kad::Behaviour::new(
                        key.public().to_peer_id(),
                        kad::store::MemoryStore::new(key.public().to_peer_id()),
                    ),
                    gossipsub,
                    health_request_response: request_response::Behaviour::<health::Codec>::new(
                        [(
                            StreamProtocol::new(health::IDENTIFIER),
                            request_response::ProtocolSupport::Outbound,
                        )],
                        request_response::Config::default(),
                    ),
                }
            })
            .map_err(|_| {
                SwarmError::BehaviourCreation("Failed to create swarm behavior.".to_string())
            })?
            .build();

        Ok((
            Network {
                action_sender,
                stream_control: swarm.behaviour().stream.new_control(),
            },
            NetworkDriver {
                swarm,
                pending_dials_map: HashMap::default(),
                pending_listen_map: HashMap::default(),
                pending_queries_map: HashMap::default(),
                pending_bootstrap: Arc::new(SetOnce::new()),
                health_outbound_requests_map: HashMap::default(),
                health_outbound_responses_map: HashMap::default(),
                health_request_handlers: Vec::new(),
                action_receiver,
                exclude_cidrs,
            },
        ))
    }
}

impl SwarmDriver<Behaviour> for NetworkDriver {
    async fn run(mut self) -> Result<(), SwarmError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                            tracing::debug!(peer_id = %peer_id, ?endpoint, "Established new connection");
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::ConnectionClosed { peer_id, endpoint, .. } => {
                            tracing::debug!(peer_id = %peer_id, ?endpoint, "Closed connection");
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { listener_id, address } => {
                            self.process_new_listen_addr(&listener_id, address).await;
                        }
                        SwarmEvent::ExternalAddrConfirmed { address, .. } => {
                            self.process_confirmed_external_addr(address);
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
                            self.process_identify_event(event);
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {id,  result, step, ..})) => {
                             self.process_kademlia_query_result(id, result, step).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Dcutr(event)) => {
                            tracing::debug!("dcutr event: {:?}", event);
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::HealthRequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, health::Codec>>::process_request_response_event(&mut self, event).await;
                        }

                        _ => {
                            tracing::debug!("Unhandled event: {:?}", event);
                        }
                    }
                },
                Some(action) = self.action_receiver.recv() => {
                    match action {
                        Action::Dial(action) => self.process_dial_action(action).await,
                        Action::Listen(action) => self.process_listen_action(action).await,
                        Action::Kademlia(action) => self.process_kademlia_action(action).await,
                        Action::HealthRequestResponse(action) =>
                            <NetworkDriver as RequestResponseDriver<Behaviour, health::Codec>>::process_request_response_action(&mut self, action).await,
                    }
                }
                else => break
            }
        }

        Ok(())
    }

    fn swarm(&mut self) -> &mut Swarm<Behaviour> {
        &mut self.swarm
    }
}

impl DialInterface for Network {
    async fn send(&self, action: DialAction) {
        if let Err(e) = self.action_sender.send(Action::Dial(action)).await {
            tracing::error!(?e, "failed to send dial action");
        }
    }
}

impl DialDriver<Behaviour> for NetworkDriver {
    fn pending_dials(&mut self) -> &mut PendingDials {
        &mut self.pending_dials_map
    }
}

impl ListenInterface for Network {
    async fn send(&self, action: ListenAction) {
        if let Err(e) = self.action_sender.send(Action::Listen(action)).await {
            tracing::error!(?e, "failed to send listen action");
        }
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(&mut self) -> &mut PendingListens {
        &mut self.pending_listen_map
    }
}

impl StreamPullInterface for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamPullReceiverInterface<DataSlice> for Network {}

impl KademliaBehavior for Behaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<kad::store::MemoryStore> {
        &mut self.kademlia
    }
}

impl KademliaDriver<Behaviour> for NetworkDriver {
    fn pending_queries(&mut self) -> &mut PendingQueries {
        &mut self.pending_queries_map
    }

    fn pending_bootstrap(&mut self) -> &mut Arc<SetOnce<()>> {
        &mut self.pending_bootstrap
    }

    fn exclude_cidrs(&self) -> Vec<hypha_network::IpNet> {
        self.exclude_cidrs.clone()
    }
}

impl KademliaInterface for Network {
    async fn send(&self, action: KademliaAction) {
        if let Err(e) = self.action_sender.send(Action::Kademlia(action)).await {
            tracing::error!(?e, "failed to send kademlia action");
        }
    }
}

impl RequestResponseBehaviour<health::Codec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<health::Codec> {
        &mut self.health_request_response
    }
}

impl RequestResponseDriver<Behaviour, health::Codec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<health::Codec> {
        &mut self.health_outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.health_outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut HealthRequestHandlers {
        &mut self.health_request_handlers
    }
}

impl RequestResponseInterface<health::Codec> for Network {
    async fn send(&self, action: RequestResponseAction<health::Codec>) {
        self.action_sender
            .send(Action::HealthRequestResponse(action))
            .await
            .expect("network driver is running");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<health::Codec>,
    ) -> Result<(), RequestResponseError> {
        self.action_sender
            .try_send(Action::HealthRequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Failed to send action".to_string()))
    }
}
