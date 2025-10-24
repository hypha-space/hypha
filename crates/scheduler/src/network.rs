//! Networking utilities for the scheduler component.
//!
//! The scheduler orchestrates workers via libp2p. This module brings together
//! the networking primitives and drives the underlying swarm.

use std::{collections::HashMap, sync::Arc};

use futures_util::stream::StreamExt;
use hypha_messages::{api, health, progress};
use hypha_network::{
    CertificateDer, CertificateRevocationListDer, IpNet, PrivateKeyDer,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    external_address::{ExternalAddressAction, ExternalAddressDriver, ExternalAddressInterface},
    gossipsub::{
        GossipsubAction, GossipsubBehaviour, GossipsubDriver, GossipsubInterface, Subscriptions,
    },
    kad::{
        KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, KademliaResult,
        PendingQueries,
    },
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterface,
    },
    stream_push::{StreamPushInterface, StreamPushSenderInterface},
    swarm::{SwarmDriver, SwarmError},
};
use hypha_telemetry::{bandwidth, metrics};
use libp2p::{
    PeerId, StreamProtocol, Swarm, SwarmBuilder,
    core::{muxing::StreamMuxerBox, transport::Transport},
    dcutr, gossipsub, identify, kad, ping, relay, request_response,
    swarm::{ConnectionId, DialError, NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use libp2p_stream as stream;
use tokio::sync::{SetOnce, mpsc, oneshot};

type HyphaRequestHandlers = Vec<RequestHandler<api::Codec>>;
type HealthRequestHandlers = Vec<RequestHandler<health::Codec>>;
type ProgressRequestHandlers = Vec<RequestHandler<progress::Codec>>;

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
    request_response: request_response::Behaviour<api::Codec>,
    health_request_response: request_response::Behaviour<health::Codec>,
    progress_request_response: request_response::Behaviour<progress::Codec>,
}

pub struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    pending_bootstrap: Arc<SetOnce<()>>,
    subscriptions: Subscriptions,
    action_receiver: mpsc::Receiver<Action>,
    outbound_requests_map: OutboundRequests<api::Codec>,
    outbound_responses_map: OutboundResponses,
    request_handlers: HyphaRequestHandlers,
    health_outbound_requests_map: OutboundRequests<health::Codec>,
    health_outbound_responses_map: OutboundResponses,
    health_request_handlers: HealthRequestHandlers,
    progress_outbound_requests_map: OutboundRequests<progress::Codec>,
    progress_outbound_responses_map: OutboundResponses,
    progress_request_handlers: ProgressRequestHandlers,
    exclude_cidrs: Vec<IpNet>,
}

#[allow(clippy::large_enum_variant)]
enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    Gossipsub(GossipsubAction),
    RequestResponse(RequestResponseAction<api::Codec>),
    HealthRequestResponse(RequestResponseAction<health::Codec>),
    ProgressRequestResponse(RequestResponseAction<progress::Codec>),
    ExternalAddress(ExternalAddressAction),
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
        let meter = metrics::global::meter();

        // Build libp2p Swarm using the derived identity and mTLS config
        let swarm = SwarmBuilder::with_existing_identity(cert_chain, private_key, ca_certs, crls)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                tls::Config::new,
                yamux::Config::default,
            )
            .map_err(|_| {
                SwarmError::TransportConfig("Failed to create TCP transport with mTLS.".to_string())
            })?
            .with_quic()
            .with_relay_client(tls::Config::new, yamux::Config::default)
            .map_err(|_| {
                SwarmError::TransportConfig("Failed to create relay client with mTLS.".to_string())
            })?
            .map_transport(|t| {
                // NOTE: Instrument transport bandwidth for metrics collection.
                bandwidth::Transport::new(t, &meter)
                    .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
            })
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
                    stream: stream::Behaviour::with_relay_policy(
                        libp2p_stream::ConnectionPolicy::IgnoreRelayed,
                    ),
                    kademlia: kad::Behaviour::new(
                        key.public().to_peer_id(),
                        kad::store::MemoryStore::new(key.public().to_peer_id()),
                    ),
                    gossipsub,
                    request_response: request_response::Behaviour::<api::Codec>::new(
                        [(
                            StreamProtocol::new(api::IDENTIFIER),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                    health_request_response: request_response::Behaviour::<health::Codec>::new(
                        [(
                            StreamProtocol::new(health::IDENTIFIER),
                            request_response::ProtocolSupport::Outbound,
                        )],
                        request_response::Config::default(),
                    ),
                    progress_request_response: request_response::Behaviour::<progress::Codec>::new(
                        [(
                            StreamProtocol::new(progress::IDENTIFIER),
                            request_response::ProtocolSupport::Full,
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
                subscriptions: HashMap::default(),
                outbound_requests_map: HashMap::default(),
                outbound_responses_map: HashMap::default(),
                request_handlers: Vec::new(),
                health_outbound_requests_map: HashMap::default(),
                health_outbound_responses_map: HashMap::default(),
                health_request_handlers: Vec::new(),
                progress_outbound_requests_map: HashMap::default(),
                progress_outbound_responses_map: HashMap::default(),
                progress_request_handlers: Vec::new(),
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
                        SwarmEvent::ConnectionEstablished { connection_id, peer_id, endpoint, .. } => {
                            tracing::debug!(peer_id = %peer_id, ?endpoint, "Established new connection");
                            self.process_connection_established(peer_id, &connection_id,).await;
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
                        SwarmEvent::Behaviour(BehaviourEvent::Gossipsub(event)) => {
                        self.process_gossipsub_event(event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::RequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, api::Codec>>::process_request_response_event(&mut self, event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::HealthRequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, health::Codec>>::process_request_response_event(&mut self, event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::ProgressRequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, progress::Codec>>::process_request_response_event(&mut self, event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Dcutr(event)) => {
                            tracing::debug!("dcutr event: {:?}", event);
                        }

                        _ => {
                            tracing::debug!("Unhandled event: {:?}", event);
                        }
                    }
                },
                Some(action) = self.action_receiver.recv() => {
                    match action {
                        Action::Dial(action) => {
                            self.process_dial_action(action).await;
                        }
                        Action::Listen(action) => {
                            self.process_listen_action(action).await;
                        }
                        Action::Kademlia(action) => {
                            self.process_kademlia_action(action).await;
                        }
                        Action::Gossipsub(action) => {
                            self.process_gossipsub_action(action).await;
                        },
                        Action::RequestResponse(action) =>
                            <NetworkDriver as RequestResponseDriver<Behaviour, api::Codec>>::process_request_response_action(&mut self, action).await,
                        Action::HealthRequestResponse(action) =>
                            <NetworkDriver as RequestResponseDriver<Behaviour, health::Codec>>::process_request_response_action(&mut self, action).await,
                        Action::ProgressRequestResponse(action) =>
                            <NetworkDriver as RequestResponseDriver<Behaviour, progress::Codec>>::process_request_response_action(&mut self, action).await,
                        Action::ExternalAddress(action) =>
                            self.process_external_address_action(action).await,
                    }
                },
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
    fn pending_dials(
        &mut self,
    ) -> &mut HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>> {
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

impl ExternalAddressDriver<Behaviour> for NetworkDriver {}

impl StreamPushInterface for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamPushSenderInterface for Network {}

impl KademliaBehavior for Behaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<kad::store::MemoryStore> {
        &mut self.kademlia
    }
}

impl KademliaDriver<Behaviour> for NetworkDriver {
    fn pending_queries(
        &mut self,
    ) -> &mut HashMap<libp2p::kad::QueryId, mpsc::Sender<KademliaResult>> {
        &mut self.pending_queries_map
    }

    fn pending_bootstrap(&mut self) -> &mut Arc<SetOnce<()>> {
        &mut self.pending_bootstrap
    }

    fn exclude_cidrs(&self) -> Vec<IpNet> {
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

impl GossipsubBehaviour for Behaviour {
    fn gossipsub(&mut self) -> &mut gossipsub::Behaviour {
        &mut self.gossipsub
    }
}

impl GossipsubDriver<Behaviour> for NetworkDriver {
    fn subscriptions(&mut self) -> &mut Subscriptions {
        &mut self.subscriptions
    }
}

impl GossipsubInterface for Network {
    async fn send(&self, action: GossipsubAction) {
        if let Err(e) = self.action_sender.send(Action::Gossipsub(action)).await {
            tracing::error!(?e, "failed to send gossipsub action");
        }
    }
}

impl RequestResponseBehaviour<api::Codec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<api::Codec> {
        &mut self.request_response
    }
}

impl RequestResponseDriver<Behaviour, api::Codec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<api::Codec> {
        &mut self.outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut HyphaRequestHandlers {
        &mut self.request_handlers
    }
}

impl RequestResponseInterface<api::Codec> for Network {
    async fn send(&self, action: RequestResponseAction<api::Codec>) {
        self.action_sender
            .send(Action::RequestResponse(action))
            .await
            .expect("network driver is running");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<api::Codec>,
    ) -> Result<(), RequestResponseError> {
        self.action_sender
            .try_send(Action::RequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Failed to send action".to_string()))
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

impl RequestResponseBehaviour<progress::Codec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<progress::Codec> {
        &mut self.progress_request_response
    }
}

impl RequestResponseDriver<Behaviour, progress::Codec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<progress::Codec> {
        &mut self.progress_outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.progress_outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut ProgressRequestHandlers {
        &mut self.progress_request_handlers
    }
}

impl RequestResponseInterface<progress::Codec> for Network {
    async fn send(&self, action: RequestResponseAction<progress::Codec>) {
        self.action_sender
            .send(Action::ProgressRequestResponse(action))
            .await
            .expect("network driver is running");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<progress::Codec>,
    ) -> Result<(), RequestResponseError> {
        self.action_sender
            .try_send(Action::ProgressRequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Failed to send action".to_string()))
    }
}

impl ExternalAddressInterface for Network {
    async fn send(&self, action: ExternalAddressAction) {
        if let Err(e) = self
            .action_sender
            .send(Action::ExternalAddress(action))
            .await
        {
            tracing::error!(?e, "failed to send external address action");
        }
    }
}
