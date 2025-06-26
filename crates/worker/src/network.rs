use std::collections::HashMap;

use futures_util::stream::StreamExt;

use hypha_network::{
    CertificateDer, CertificateRevocationListDer, PrivateKeyDer, cbor_codec, cert,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    error::HyphaError,
    gossipsub::{
        GossipsubAction, GossipsubBehaviour, GossipsubDriver, GossipsubInterface, Subscriptions,
    },
    kad::{KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, PendingQueries},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    mtls,
    request_response::{
        OutboundRequests, OutboundResponses, RequestHandler, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseError,
        RequestResponseInterface,
    },
    stream::{StreamInterface, StreamSenderInterface},
    swarm::SwarmDriver,
};
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder, dcutr, gossipsub, identify, kad, ping, relay,
    request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, yamux,
};
use libp2p_stream as stream;
use tokio::sync::mpsc;

type HyphaCodec = cbor_codec::Codec<hypha_api::Request, hypha_api::Response>;
type HyphaRequestHandlers = Vec<RequestHandler<HyphaCodec>>;

#[derive(Clone)]
pub(crate) struct Network {
    action_sender: mpsc::Sender<Action>,
    stream_control: stream::Control,
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    relay_client: relay::client::Behaviour,
    dcutr: dcutr::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    request_response: request_response::Behaviour<HyphaCodec>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    subscriptions: Subscriptions,
    action_receiver: mpsc::Receiver<Action>,
    outbound_requests_map: OutboundRequests<HyphaCodec>,
    outbound_responses_map: OutboundResponses,
    request_handlers: HyphaRequestHandlers,
}

enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    Gossipsub(GossipsubAction),
    RequestResponse(RequestResponseAction<HyphaCodec>),
}

impl Network {
    pub fn create(
        cert_chain: Vec<CertificateDer<'static>>,
        private_key: PrivateKeyDer<'static>,
        ca_certs: Vec<CertificateDer<'static>>,
        crls: Vec<CertificateRevocationListDer<'static>>,
    ) -> Result<(Self, NetworkDriver), HyphaError> {
        let (action_sender, action_receiver) = mpsc::channel(5);

        // Create a libp2p keypair from the certificate and private key
        let identity = cert::identity_from_private_key(&private_key)
            .map_err(|e| HyphaError::SwarmError(format!("Failed to create identity: {}", e)))?;

        // Create mTLS config (crypto provider is initialized in mtls::Config::new)
        let mtls_config = mtls::Config::try_new(cert_chain, private_key, ca_certs, crls)
            .map_err(|e| HyphaError::SwarmError(format!("Failed to create mTLS config: {}", e)))?;

        let swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                {
                    let mtls_config = mtls_config.clone();
                    move |_: &_| Ok(mtls_config)
                },
                yamux::Config::default,
            )
            .map_err(|_: Box<dyn std::error::Error>| {
                HyphaError::SwarmError("Failed to create TCP transport.".to_string())
            })?
            .with_relay_client(
                {
                    let mtls_config = mtls_config.clone();
                    move |_: &_| Ok(mtls_config)
                },
                yamux::Config::default,
            )
            .map_err(|_: Box<dyn std::error::Error>| {
                HyphaError::SwarmError("Failed to create relay client with mTLS.".to_string())
            })?
            .with_behaviour(|key, relay_client| Behaviour {
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
                gossipsub: gossipsub::Behaviour::new(
                    gossipsub::MessageAuthenticity::Signed(key.clone()),
                    gossipsub::Config::default(),
                )
                .unwrap(),
                request_response: request_response::Behaviour::<HyphaCodec>::new(
                    [(
                        StreamProtocol::new("/hypha-api/0.0.1"),
                        request_response::ProtocolSupport::Full,
                    )],
                    request_response::Config::default(),
                ),
            })
            .map_err(|_| HyphaError::SwarmError("Failed to create swarm behavior.".to_string()))?
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
                subscriptions: HashMap::default(),
                outbound_requests_map: HashMap::default(),
                outbound_responses_map: HashMap::default(),
                request_handlers: Vec::new(),
                action_receiver,
            },
        ))
    }
}

impl SwarmDriver<Behaviour> for NetworkDriver {
    async fn run(mut self) -> Result<(), HyphaError> {
        loop {
            tokio::select! {
                event = self.swarm.select_next_some() => {
                    match event {
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, .. } => {
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { listener_id, address } => {
                            tracing::info!(address=%address, "New listen address");
                            self.process_new_listen_addr(&listener_id).await;
                        }
                        SwarmEvent::ExternalAddrConfirmed { address, .. } => {
                            tracing::info!("External address confirmed: {:?}", address);
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
                            self.process_request_response_event(event).await;
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
                        Action::Gossipsub(action) =>
                            self.process_gossipsub_action(action).await,
                        Action::RequestResponse(action) =>
                            self.process_request_response_action(action).await,
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
        self.action_sender.send(Action::Dial(action)).await.unwrap();
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
            .unwrap();
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(&mut self) -> &mut PendingListens {
        &mut self.pending_listen_map
    }
}

impl StreamInterface for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamSenderInterface for Network {}

impl KademliaBehavior for Behaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<kad::store::MemoryStore> {
        &mut self.kademlia
    }
}

impl KademliaDriver<Behaviour> for NetworkDriver {
    fn pending_queries(&mut self) -> &mut PendingQueries {
        &mut self.pending_queries_map
    }
}

impl KademliaInterface for Network {
    async fn send(&self, action: KademliaAction) {
        self.action_sender
            .send(Action::Kademlia(action))
            .await
            .unwrap();
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
            .unwrap();
    }
}

impl RequestResponseBehaviour<HyphaCodec> for Behaviour {
    fn request_response(&mut self) -> &mut libp2p::request_response::Behaviour<HyphaCodec> {
        &mut self.request_response
    }
}

impl RequestResponseDriver<Behaviour, HyphaCodec> for NetworkDriver {
    fn outbound_requests(&mut self) -> &mut OutboundRequests<HyphaCodec> {
        &mut self.outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses_map
    }

    fn request_handlers(&mut self) -> &mut HyphaRequestHandlers {
        &mut self.request_handlers
    }
}

impl RequestResponseInterface<HyphaCodec> for Network {
    async fn send(&self, action: RequestResponseAction<HyphaCodec>) {
        self.action_sender
            .send(Action::RequestResponse(action))
            .await
            .unwrap();
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
