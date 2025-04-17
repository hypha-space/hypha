use std::{collections::HashMap, io::Error as IoError, sync::Arc, time::Duration};

use futures_util::stream::StreamExt;
use hypha_network::{
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    error::HyphaError,
    kad::{
        KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, KademliaResult,
        PendingQueries,
    },
    listen::{ListenAction, ListenDriver, ListenInterface, PendingListens},
    stream::{StreamInterface, StreamReceiverInterface},
    swarm::SwarmDriver,
};
use libp2p::PeerId;
use libp2p::{
    Swarm, SwarmBuilder, TransportError,
    core::transport::ListenerId,
    identify, identity, kad, ping, relay,
    swarm::{ConnectionId, DialError, NetworkBehaviour, SwarmEvent},
    tcp, tls, yamux,
};
use libp2p_stream as stream;
use tokio::sync::{Mutex, mpsc, oneshot};

#[derive(Clone)]
pub(crate) struct Network {
    action_sender: mpsc::Sender<Action>,
    stream_control: stream::Control,
}

#[derive(NetworkBehaviour)]
pub(crate) struct Behaviour {
    ping: ping::Behaviour,
    identify: identify::Behaviour,
    relay: relay::Behaviour,
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_listen_map: PendingListens,
    pending_queries_map: PendingQueries,
    action_receiver: mpsc::Receiver<Action>,
}

enum Action {
    Dial(DialAction),
    Listen(ListenAction),
    Kademlia(KademliaAction),
}

impl Network {
    pub fn create(identity: identity::Keypair) -> Result<(Self, NetworkDriver), HyphaError> {
        let (action_sender, action_receiver) = mpsc::channel(5);
        let mut kademlia_config = kad::Config::default();

        kademlia_config.set_provider_publication_interval(Some(Duration::from_secs(10)));

        let mut swarm = SwarmBuilder::with_existing_identity(identity)
            .with_tokio()
            .with_tcp(
                tcp::Config::default(),
                tls::Config::new,
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
                action_sender,
                stream_control: swarm.behaviour().stream.new_control(),
            },
            NetworkDriver {
                swarm,
                pending_dials_map: Arc::new(Mutex::new(HashMap::default())),
                pending_listen_map: Arc::new(Mutex::new(HashMap::default())),
                pending_queries_map: Arc::new(Mutex::new(HashMap::default())),
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
                    }
                }
            }
        }
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
    fn pending_dials(
        &self,
    ) -> Arc<Mutex<HashMap<ConnectionId, oneshot::Sender<Result<PeerId, DialError>>>>> {
        self.pending_dials_map.clone()
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
    fn pending_listens(
        &self,
    ) -> Arc<Mutex<HashMap<ListenerId, oneshot::Sender<Result<(), TransportError<IoError>>>>>> {
        self.pending_listen_map.clone()
    }
}

impl StreamInterface for Network {
    fn stream_control(&self) -> stream::Control {
        self.stream_control.clone()
    }
}

impl StreamReceiverInterface for Network {}

impl KademliaBehavior for Behaviour {
    fn kademlia(&mut self) -> &mut kad::Behaviour<kad::store::MemoryStore> {
        &mut self.kademlia
    }
}

impl KademliaDriver<Behaviour> for NetworkDriver {
    fn pending_queries(&self) -> Arc<Mutex<HashMap<kad::QueryId, mpsc::Sender<KademliaResult>>>> {
        self.pending_queries_map.clone()
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
