use std::{collections::HashMap, sync::Arc};

use futures_util::stream::StreamExt;
use hypha_config::NetworkConfig;
use hypha_network::{
    CertificateDer, CertificateRevocationListDer, IpNet, PrivateKeyDer,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    external_address::{ExternalAddressAction, ExternalAddressDriver, ExternalAddressInterface},
    kad::{KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, PendingQueries},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterface,
    },
    swarm::{SwarmDriver, SwarmError},
};
use hypha_telemetry::{metrics, rtt};
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder, identify, kad, ping, request_response,
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
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    health_request_response: request_response::Behaviour<HealthCodec>,
}

pub struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    pending_bootstrap: Arc<SetOnce<()>>,
    action_receiver: mpsc::Receiver<Action>,
    health_outbound_requests_map: OutboundRequests<HealthCodec>,
    health_outbound_responses_map: OutboundResponses,
    health_request_handlers: Vec<RequestHandler<HealthCodec>>,
    exclude_cidrs: Vec<IpNet>,
    rtt_metrics: rtt::RttMetrics,
}

#[allow(clippy::large_enum_variant)]
enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    ExternalAddress(ExternalAddressAction),
    HealthRequestResponse(RequestResponseAction<HealthCodec>),
}

type HealthCodec = hypha_messages::health::Codec;

impl Network {
    pub fn create(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
        exclude_cidrs: Vec<IpNet>,
        network_config: &NetworkConfig,
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
                .with_quic_config(|mut c| {
                    // NOTE: Flow-control windows are sized from the configured bandwidth-delay product.
                    let bdp_bytes: u64 = (network_config.bandwidth_mbps() * 1_000_000 / 8)
                        * network_config.rtt_ms()
                        / 1000;

                    let max_stream_data =
                        ((3_u64 * bdp_bytes).next_power_of_two()).min(u32::MAX as u64) as u32;
                    let max_connection_data =
                        ((4_u64 * bdp_bytes).next_power_of_two()).min(u32::MAX as u64) as u32;

                    c.max_stream_data = max_stream_data;
                    c.max_connection_data = max_connection_data;
                    c.max_connection_send_data = Some(max_connection_data);
                    c.handshake_timeout = network_config.handshake_timeout();
                    c
                })
                .with_dns()
                .map_err(|_| SwarmError::TransportConfig("Failed to setup DNS".to_string()))?
                .with_behaviour(|key| Behaviour {
                    ping: ping::Behaviour::new(ping::Config::new()),
                    identify: identify::Behaviour::new(identify::Config::new(
                        "/hypha-identify/0.0.1".to_string(),
                        key.public(),
                    )),
                    kademlia: kad::Behaviour::new(
                        key.public().to_peer_id(),
                        kad::store::MemoryStore::new(key.public().to_peer_id()),
                    ),
                    health_request_response: request_response::Behaviour::<HealthCodec>::new(
                        [(
                            StreamProtocol::new(hypha_messages::health::IDENTIFIER),
                            request_response::ProtocolSupport::Outbound,
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
            .set_mode(Some(kad::Mode::Client));

        Ok((
            Network { action_sender },
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
                rtt_metrics: rtt::RttMetrics::new(&meter),
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
                        SwarmEvent::NewListenAddr { listener_id, address } => {
                            self.process_new_listen_addr(&listener_id, address).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(event)) => {
                            self.process_identify_event(event);
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {id,  result, step, ..})) => {
                            self.process_kademlia_query_result(id, result, step).await;
                         }
                        SwarmEvent::Behaviour(BehaviourEvent::HealthRequestResponse(event)) => {
                            <NetworkDriver as RequestResponseDriver<Behaviour, HealthCodec>>::process_request_response_event(&mut self, event).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Ping(ping::Event { peer, result, .. })) => {
                            if let Ok(rtt) = result {
                                self.rtt_metrics.record(&peer, rtt);
                            }
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
                        Action::ExternalAddress(action) => {
                            self.process_external_address_action(action).await;
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

    fn exclude_cidrs(&self) -> &[IpNet] {
        &self.exclude_cidrs
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

impl RequestResponseBehaviour<HealthCodec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<HealthCodec> {
        &mut self.health_request_response
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

    fn exclude_cidrs(&self) -> Vec<IpNet> {
        self.exclude_cidrs.clone()
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
