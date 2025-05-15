use std::collections::HashMap;

use futures_util::stream::StreamExt;
use hypha_api;
use hypha_network::{
    cbor_codec,
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    error::HyphaError,
    gossipsub::{
        GossipsubAction, GossipsubBehaviour, GossipsubDriver, GossipsubInterface, Subscriptions,
    },
    kad::{KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, PendingQueries},
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    request_response::{
        InboundRequestSender, OutboundRequests, OutboundResponses, RequestResponseAction,
        RequestResponseBehaviour, RequestResponseDriver, RequestResponseInterface,
    },
    stream::{StreamInterface, StreamReceiverInterface, StreamSenderInterface},
    swarm::SwarmDriver,
};
use libp2p::{
    StreamProtocol, Swarm, SwarmBuilder, gossipsub, identify, identity, kad, ping,
    request_response,
    swarm::{NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use libp2p_stream as stream;
use tokio::sync::{OnceCell, mpsc};

#[derive(Clone)]
pub(crate) struct Network {
    action_sender: mpsc::Sender<Action>,
    stream_control: stream::Control,
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
    gossipsub: gossipsub::Behaviour,
    request_response:
        request_response::Behaviour<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    subscriptions: Subscriptions,
    action_receiver: mpsc::Receiver<Action>,
    outbound_requests_map:
        OutboundRequests<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>,
    outbound_responses_map: OutboundResponses,
    inbound_request_sender:
        OnceCell<InboundRequestSender<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>>,
}

enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
    Gossipsub(GossipsubAction),
    RequestResponse(
        RequestResponseAction<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>,
    ),
}

impl Network {
    pub fn create(identity: identity::Keypair) -> Result<(Self, NetworkDriver), HyphaError> {
        let (action_sender, action_receiver) = mpsc::channel(5);

        let swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                tls::Config::new,
                yamux::Config::default,
            )
            .map_err(|_| HyphaError::SwarmError("Failed to create TCP transport.".to_string()))?
            .with_quic()
            .with_behaviour(|key| Behaviour {
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/hypha-identify/0.0.1".to_string(),
                    key.public(),
                )),
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
                request_response: request_response::Behaviour::<
                    cbor_codec::Codec<hypha_api::Request, hypha_api::Response>,
                >::new(
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
                inbound_request_sender: OnceCell::new(),
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
                        SwarmEvent::Behaviour(BehaviourEvent::Identify(identify::Event::Received { peer_id, info, .. })) => {
                            // Add known addresses of peers to the Kademlia routing table
                            tracing::debug!(peer_id=%peer_id, info=?info, "Adding address to Kademlia routing table");
                            for addr in info.listen_addrs {
                                self.swarm.behaviour_mut().kademlia.add_address(&peer_id, addr);
                            }
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
impl StreamReceiverInterface for Network {}

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

impl RequestResponseBehaviour<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>
    for Behaviour
{
    fn request_response(
        &mut self,
    ) -> &mut libp2p::request_response::Behaviour<
        cbor_codec::Codec<hypha_api::Request, hypha_api::Response>,
    > {
        &mut self.request_response
    }
}

impl RequestResponseDriver<Behaviour, cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>
    for NetworkDriver
{
    fn outbound_requests(
        &mut self,
    ) -> &mut OutboundRequests<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>> {
        &mut self.outbound_requests_map
    }

    fn outbound_responses(&mut self) -> &mut OutboundResponses {
        &mut self.outbound_responses_map
    }

    fn inbound_request_sender(
        &self,
    ) -> &OnceCell<InboundRequestSender<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>>
    {
        &self.inbound_request_sender
    }
}

impl RequestResponseInterface<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>
    for Network
{
    async fn send(
        &self,
        action: RequestResponseAction<cbor_codec::Codec<hypha_api::Request, hypha_api::Response>>,
    ) {
        self.action_sender
            .send(Action::RequestResponse(action))
            .await
            .unwrap();
    }
}
