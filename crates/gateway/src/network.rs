//! Networking utilities for the gateway component.
//!
//! This module wires together the various networking primitives to run the
//! gateway's event loop.

use std::{collections::HashMap, sync::Arc};

use futures_util::stream::StreamExt;
use hypha_network::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    external_address::{ExternalAddressAction, ExternalAddressDriver, ExternalAddressInterface},
    gossipsub::{
        GossipsubAction, GossipsubBehaviour, GossipsubDriver, GossipsubInterface, Subscriptions,
    },
    kad::{KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, PendingQueries},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterface,
    },
    swarm::{SwarmDriver, SwarmError},
};
use hypha_telemetry::{bandwidth, metrics};
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder,
    core::{muxing::StreamMuxerBox, transport::Transport},
    gossipsub, identify, kad, ping, relay, request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use tokio::sync::{SetOnce, mpsc};

#[derive(Clone)]
pub struct Network {
    action_sender: mpsc::Sender<Action>,
}

#[derive(NetworkBehaviour)]
pub struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    relay: relay::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    request_response: request_response::Behaviour<HyphaCodec>,
    health_request_response: request_response::Behaviour<HealthCodec>,
}

pub struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    pending_bootstrap: Arc<SetOnce<()>>,
    subscriptions: Subscriptions,
    action_receiver: mpsc::Receiver<Action>,
    outbound_requests_map: OutboundRequests<HyphaCodec>,
    outbound_responses_map: OutboundResponses,
    request_handlers: Vec<RequestHandler<HyphaCodec>>,
    health_outbound_requests_map: OutboundRequests<HealthCodec>,
    health_outbound_responses_map: OutboundResponses,
    health_request_handlers: Vec<RequestHandler<HealthCodec>>,
}

#[allow(clippy::large_enum_variant)]
enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    Gossipsub(GossipsubAction),
    ExternalAddress(ExternalAddressAction),
    RequestResponse(RequestResponseAction<HyphaCodec>),
    HealthRequestResponse(RequestResponseAction<HealthCodec>),
}

type HyphaCodec = hypha_messages::api::Codec;
type HealthCodec = hypha_messages::health::Codec;

impl Network {
    pub fn create(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> Result<(Self, NetworkDriver), SwarmError> {
        let (action_sender, action_receiver) = mpsc::channel(5);
        let meter = metrics::global::meter();

        let mut swarm =
            SwarmBuilder::with_existing_identity(cert_chain, private_key, ca_certs, crls)
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
                .map_transport(|t| {
                    bandwidth::Transport::new(t, &meter)
                        .map(|(peer, muxer), _| (peer, StreamMuxerBox::new(muxer)))
                })
                .with_behaviour(|key| Behaviour {
                    relay: relay::Behaviour::new(key.public().to_peer_id(), Default::default()),
                    ping: ping::Behaviour::new(ping::Config::new()),
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/hypha-identify/0.0.1".to_string(),
                        key.public(),
                    )),
                    kademlia: kad::Behaviour::new(
                        key.public().to_peer_id(),
                        kad::store::MemoryStore::new(key.public().to_peer_id()),
                    ),
                    gossipsub: gossipsub::Behaviour::new(
                        gossipsub::MessageAuthenticity::Signed(key.clone()),
                        gossipsub::Config::default(),
                    )
                    .expect("Failed to create gossipsub behaviour"),
                    request_response: request_response::Behaviour::<HyphaCodec>::new(
                        [(
                            StreamProtocol::new("/hypha-api/0.0.1"),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                    health_request_response: request_response::Behaviour::<HealthCodec>::new(
                        [(
                            StreamProtocol::new(hypha_messages::health::IDENTIFIER),
                            request_response::ProtocolSupport::Full,
                        )],
                        request_response::Config::default(),
                    ),
                })
                .map_err(|_| {
                    SwarmError::BehaviourCreation("Failed to create swarm behavior.".to_string())
                })?
                // TODO: Tune swarm configuration
                .with_swarm_config(|config| config)
                .build();

        swarm
            .behaviour_mut()
            .kademlia
            .set_mode(Some(kad::Mode::Server));

        Ok((
            Network { action_sender },
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
                action_receiver,
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
                        SwarmEvent::ConnectionEstablished { connection_id, peer_id, .. } => {
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { listener_id, .. } => {
                            self.process_new_listen_addr(&listener_id).await;
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
                            <NetworkDriver as RequestResponseDriver<Behaviour, HyphaCodec>>::process_request_response_event(&mut self, event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::HealthRequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, HealthCodec>>::process_request_response_event(&mut self, event).await;
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
                        },
                        Action::Listen(action) => { self.process_listen_action(action).await; },
                        Action::Kademlia(action) => { self.process_kademlia_action(action).await; },
                        Action::Gossipsub(action) => {
                            self.process_gossipsub_action(action).await;
                        },
                        Action::ExternalAddress(action) => {
                            self.process_external_address_action(action).await;
                        }
                        Action::RequestResponse(action) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, HyphaCodec>>::process_request_response_action(&mut self, action).await;
                        }
                        Action::HealthRequestResponse(action) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, HealthCodec>>::process_request_response_action(&mut self, action).await;
                        }
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
        self.action_sender
            .send(Action::Dial(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }
}

impl DialDriver<Behaviour> for NetworkDriver {
    fn pending_dials(&mut self) -> &mut PendingDials {
        &mut self.pending_dials_map
    }
}

impl ListenInterface for Network {
    async fn send(&self, action: ListenAction) {
        self.action_sender
            .send(Action::Listen(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(&mut self) -> &mut PendingListens {
        &mut self.pending_listen_map
    }
}

impl ExternalAddressDriver<Behaviour> for NetworkDriver {}

impl ExternalAddressInterface for Network {
    async fn send(&self, action: ExternalAddressAction) {
        self.action_sender
            .send(Action::ExternalAddress(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }
}

impl RequestResponseBehaviour<HyphaCodec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<HyphaCodec> {
        &mut self.request_response
    }
}

impl RequestResponseBehaviour<HealthCodec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<HealthCodec> {
        &mut self.health_request_response
    }
}

impl RequestResponseDriver<Behaviour, HyphaCodec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<HyphaCodec> {
        &mut self.outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut Vec<RequestHandler<HyphaCodec>> {
        &mut self.request_handlers
    }
}

impl RequestResponseDriver<Behaviour, HealthCodec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<HealthCodec> {
        &mut self.health_outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.health_outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut Vec<RequestHandler<HealthCodec>> {
        &mut self.health_request_handlers
    }
}

impl RequestResponseInterface<HyphaCodec> for Network {
    async fn send(&self, action: RequestResponseAction<HyphaCodec>) {
        self.action_sender
            .send(Action::RequestResponse(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<HyphaCodec>,
    ) -> Result<(), RequestResponseError> {
        self.action_sender
            .try_send(Action::RequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Failed to send action".to_string()))
    }
}

impl RequestResponseInterface<HealthCodec> for Network {
    async fn send(&self, action: RequestResponseAction<HealthCodec>) {
        self.action_sender
            .send(Action::HealthRequestResponse(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }

    fn try_send(
        &self,
        action: RequestResponseAction<HealthCodec>,
    ) -> Result<(), RequestResponseError> {
        self.action_sender
            .try_send(Action::HealthRequestResponse(action))
            .map_err(|_| RequestResponseError::Other("Failed to send action".to_string()))
    }
}

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
}

impl KademliaInterface for Network {
    async fn send(&self, action: KademliaAction) {
        self.action_sender
            .send(Action::Kademlia(action))
            .await
            .expect("network driver should be running and able to receive actions");
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
        self.action_sender
            .send(Action::Gossipsub(action))
            .await
            .expect("network driver should be running and able to receive actions");
    }
}
