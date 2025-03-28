use std::{collections::HashMap, sync::Arc};

use futures_util::stream::StreamExt;
use hypha_network::{
    dial::{DialAction, DialDriver, DialInterface, PendingDials},
    error::HyphaError,
    kad::{
        KademliaAction, KademliaBehavior, KademliaDriver, KademliaInterface, KademliaResult,
        PendingQueries,
    },
    stream::{StreamInterface, StreamSenderInterface},
    swarm::SwarmDriver,
};
use libp2p::{
    PeerId, Swarm, SwarmBuilder, identify, identity, kad, ping,
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
    stream: stream::Behaviour,
    kademlia: kad::Behaviour<kad::store::MemoryStore>,
}

pub(crate) struct NetworkDriver {
    swarm: Swarm<Behaviour>,
    pending_dials_map: PendingDials,
    pending_queries_map: PendingQueries,
    action_receiver: mpsc::Receiver<Action>,
}

enum Action {
    Dial(DialAction),
    Kademlia(KademliaAction),
}

impl Network {
    pub fn create(identity: identity::Keypair) -> Result<(Self, NetworkDriver), HyphaError> {
        let (action_sender, action_receiver) = mpsc::channel(5);

        let mut swarm = SwarmBuilder::with_existing_identity(identity)
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
            })
            .map_err(|_| HyphaError::SwarmError("Failed to create swarm behavior.".to_string()))?
            .build();

        swarm
            .behaviour_mut()
            .kademlia
            .set_mode(Some(kad::Mode::Client));

        Ok((
            Network {
                action_sender,
                stream_control: swarm.behaviour().stream.new_control(),
            },
            NetworkDriver {
                swarm,
                pending_dials_map: Arc::new(Mutex::new(HashMap::default())),
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
                Some(action) = self.action_receiver.recv() => {
                    match action {
                        Action::Dial(action) => self.process_dial_action(action).await,
                        Action::Kademlia(action) => self.process_kademlia_action(action).await,
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
    fn pending_queries(
        &self,
    ) -> Arc<Mutex<HashMap<libp2p::kad::QueryId, mpsc::Sender<KademliaResult>>>> {
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
