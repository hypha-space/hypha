use std::collections::HashMap;
use std::sync::Arc;

use futures::stream::StreamExt;

use libp2p::PeerId;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use libp2p::Swarm;
use libp2p::SwarmBuilder;
use libp2p::identify;
use libp2p::identity;
use libp2p::kad;
use libp2p::noise;
use libp2p::ping;
use libp2p::swarm::ConnectionId;
use libp2p::swarm::DialError;
use libp2p::swarm::NetworkBehaviour;
use libp2p::swarm::SwarmEvent;
use libp2p::tcp;
use libp2p::yamux;
use libp2p_stream as stream;

use hypha_network::dial::DialAction;
use hypha_network::dial::DialDriver;
use hypha_network::dial::DialInterface;
use hypha_network::error::HyphaError;
use hypha_network::kad::KademliaAction;
use hypha_network::kad::KademliaBehavior;
use hypha_network::kad::KademliaDriver;
use hypha_network::kad::KademliaInterface;
use hypha_network::kad::KademliaResult;
use hypha_network::stream::StreamInterface;
use hypha_network::stream::StreamSenderInterface;
use hypha_network::swarm::SwarmDriver;
use hypha_network::swarm::SwarmInterface;

#[derive(Clone)]
pub(crate) struct Network {
    dial_action_sender: Sender<DialAction>,
    kademlia_action_sender: Sender<KademliaAction>,
    stream_control: stream::Control,
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map:
        Arc<Mutex<HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>>>,
    dial_action_receiver: Receiver<DialAction>,
    pending_queries_map: Arc<Mutex<HashMap<kad::QueryId, Sender<KademliaResult>>>>,
    kademlia_action_receiver: Receiver<KademliaAction>,
}

impl SwarmInterface<Behaviour> for Network {
    type Driver = NetworkDriver;

    fn create(identity: identity::Keypair) -> Result<(Self, Self::Driver), HyphaError> {
        let (dial_action_sender, dial_action_receiver) = channel(5);
        let (kademlia_action_sender, kademlia_action_receiver) = channel(5);

        let mut swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
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
            })
            .map_err(|_| HyphaError::SwarmError("Failed to create swarm behavior.".to_string()))?
            .build();

        swarm
            .behaviour_mut()
            .kademlia
            .set_mode(Some(kad::Mode::Client));

        Ok((
            Network {
                dial_action_sender,
                stream_control: swarm.behaviour().stream.new_control(),
                kademlia_action_sender,
            },
            NetworkDriver {
                swarm,
                dial_action_receiver,
                pending_dials_map: Arc::new(Mutex::new(HashMap::default())),
                kademlia_action_receiver,
                pending_queries_map: Arc::new(Mutex::new(HashMap::default())),
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
                        SwarmEvent::ConnectionEstablished { peer_id, connection_id, endpoint, .. } => {
                            // TODO: Move into the kademlia module also I wonder whether we really should add all and instead may just want to add the gateway.
                            self.swarm.behaviour_mut().kademlia.add_address(&peer_id, endpoint.get_remote_address().clone());
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {id,  result, step, ..})) => {
                             self.process_kademlia_query_result(id, result, step).await;
                         }
                        _ => {
                            tracing::debug!("Unhandled event: {:?}", event);
                        }
                    }
                },
                Some(action) = self.dial_action_receiver.recv() => {
                    self.process_dial_action(action).await;
                },
                Some(action) = self.kademlia_action_receiver.recv() => {
                    self.process_kademlia_action(action).await;
                }
            }
        }
    }

    fn swarm(&mut self) -> &mut Swarm<Behaviour> {
        &mut self.swarm
    }
}

impl DialInterface<Behaviour> for Network {
    fn dial_action_sender(&self) -> Sender<DialAction> {
        self.dial_action_sender.clone()
    }
}

impl DialDriver<Behaviour> for NetworkDriver {
    fn pending_dials(
        &self,
    ) -> Arc<Mutex<HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>>> {
        self.pending_dials_map.clone()
    }
}

impl StreamInterface<Behaviour> for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamSenderInterface<Behaviour> for Network {}

impl KademliaBehavior for Behaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<kad::store::MemoryStore> {
        &mut self.kademlia
    }
}

impl KademliaDriver<Behaviour> for NetworkDriver {
    fn pending_queries(&self) -> Arc<Mutex<HashMap<libp2p::kad::QueryId, Sender<KademliaResult>>>> {
        self.pending_queries_map.clone()
    }
}
impl KademliaInterface<Behaviour> for Network {
    fn kademlia_action_sender(&self) -> Sender<KademliaAction> {
        self.kademlia_action_sender.clone()
    }
}
