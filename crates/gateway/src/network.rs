use std::collections::HashMap;
use std::io::Error as IoError;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::StreamExt;

use libp2p::PeerId;
use tokio::sync::Mutex;
use tokio::sync::mpsc::Receiver;
use tokio::sync::mpsc::Sender;
use tokio::sync::mpsc::channel;
use tokio::sync::oneshot;

use libp2p::Swarm;
use libp2p::SwarmBuilder;
use libp2p::TransportError;
use libp2p::core::transport::ListenerId;
use libp2p::identify;
use libp2p::identity;
use libp2p::kad;
use libp2p::mdns;
use libp2p::noise;
use libp2p::ping;
use libp2p::relay;
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
use hypha_network::listen::ListenAction;
use hypha_network::listen::ListenDriver;
use hypha_network::listen::ListenInterface;
use hypha_network::stream::StreamInterface;
use hypha_network::stream::StreamReceiverInterface;
use hypha_network::swarm::SwarmDriver;
use hypha_network::swarm::SwarmInterface;

#[derive(Clone)]
pub(crate) struct Network {
    dial_action_sender: Sender<DialAction>,
    listen_action_sender: Sender<ListenAction>,
    stream_control: stream::Control,
    kademlia_action_sender: Sender<KademliaAction>,
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    ping: ping::Behaviour,
    mdns: mdns::tokio::Behaviour,
    identify: identify::Behaviour,
    relay: relay::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map:
        Arc<Mutex<HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>>>,
    dial_action_receiver: Receiver<DialAction>,
    pending_listen_map:
        Arc<Mutex<HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>>>,
    listen_action_receiver: Receiver<ListenAction>,
    pending_queries_map: Arc<Mutex<HashMap<kad::QueryId, Sender<KademliaResult>>>>,
    kademlia_action_receiver: Receiver<KademliaAction>,
}

impl SwarmInterface<Behaviour> for Network {
    type Driver = NetworkDriver;

    fn create(identity: identity::Keypair) -> Result<(Self, Self::Driver), HyphaError> {
        let (dial_action_sender, dial_action_receiver) = channel(5);
        let (listen_action_sender, listen_action_receiver) = channel(5);
        let (kademlia_action_sender, kademlia_action_receiver) = channel(5);
        let mut kademlia_config = kad::Config::default();

        kademlia_config.set_provider_publication_interval(Some(Duration::from_secs(10)));

        let mut swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                noise::Config::new,
                yamux::Config::default,
            )
            .map_err(|_| HyphaError::SwarmError("Failed to create TCP transport.".to_string()))?
            // TODO: tune quic configuration
            .with_quic_config(|config| config)
            .with_behaviour(|key| Behaviour {
                relay: relay::Behaviour::new(key.public().to_peer_id(), Default::default()),
                ping: ping::Behaviour::new(ping::Config::new()),
                identify: identify::Behaviour::new(identify::Config::new(
                    "/hypha-identify/0.0.1".to_string(),
                    key.public(),
                )),
                stream: stream::Behaviour::new(),
                mdns: mdns::tokio::Behaviour::new(
                    mdns::Config::default(),
                    key.public().to_peer_id(),
                )
                .expect("Failed to create mDNS behavior."),
                kademlia: kad::Behaviour::with_config(
                    key.public().to_peer_id(),
                    kad::store::MemoryStore::new(key.public().to_peer_id()),
                    kademlia_config,
                ),
            })
            .map_err(|_| HyphaError::SwarmError("Failed to create swarm behavior.".to_string()))?
            // TODO: Tune swarm configuration
            .with_swarm_config(|config| config)
            .build();

        swarm
            .behaviour_mut()
            .kademlia
            .set_mode(Some(kad::Mode::Server));

        Ok((
            Network {
                dial_action_sender,
                listen_action_sender,
                stream_control: swarm.behaviour().stream.new_control(),
                kademlia_action_sender,
            },
            NetworkDriver {
                swarm,
                dial_action_receiver,
                pending_dials_map: Arc::new(Mutex::new(HashMap::default())),
                listen_action_receiver,
                pending_listen_map: Arc::new(Mutex::new(HashMap::default())),
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
                        SwarmEvent::ConnectionEstablished { connection_id, peer_id, .. } => {
                            self.process_connection_established(peer_id, &connection_id).await;
                        }
                        SwarmEvent::OutgoingConnectionError { connection_id, error, .. } => {
                            self.process_connection_error(&connection_id, error).await;
                        }
                        SwarmEvent::NewListenAddr { listener_id, address } => {
                            tracing::info!(address=%address, "New listen address");
                            self.process_new_listen_addr(&listener_id).await;
                        }
                        SwarmEvent::Behaviour(BehaviourEvent::Kademlia(kad::Event::OutboundQueryProgressed {id,  result, step, ..})) => {
                             self.process_kademlia_query_result(id, result, step).await;
                         }
                         SwarmEvent::Behaviour(BehaviourEvent::Mdns(mdns::Event::Discovered(list))) => {
                             for (peer_id, multiaddr) in list {
                                 // TODO: Move into the kademlia module
                                 self.swarm.behaviour_mut().kademlia.add_address(&peer_id, multiaddr);
                             }
                         }
                        _ => {
                            tracing::debug!("Unhandled event: {:?}", event);
                        }
                    }
                },
                Some(action) = self.dial_action_receiver.recv() => {
                    self.process_dial_action(action).await;
                },
                Some(action) = self.listen_action_receiver.recv() => {
                    self.process_listen_action(action).await;
                }
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

impl ListenInterface<Behaviour> for Network {
    fn listen_action_sender(&self) -> Sender<ListenAction> {
        self.listen_action_sender.clone()
    }
}

impl ListenDriver<Behaviour> for NetworkDriver {
    fn pending_listens(
        &self,
    ) -> Arc<Mutex<HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>>> {
        self.pending_listen_map.clone()
    }
}

impl StreamInterface<Behaviour> for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamReceiverInterface<Behaviour> for Network {}

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
