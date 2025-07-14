use libp2p::{Swarm, swarm::NetworkBehaviour};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum SwarmError {
    #[error("Transport configuration failed: {0}")]
    TransportConfig(String),
    #[error("Behaviour creation failed: {0}")]
    BehaviourCreation(String),
    #[error("Swarm configuration failed: {0}")]
    SwarmConfig(String),
}

#[allow(async_fn_in_trait)]
pub trait SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
    Self: Sized,
{
    fn swarm(&mut self) -> &mut Swarm<TBehavior>;

    /// Run the swarm driver, processing incoming actions and managing the swarm.
    async fn run(self) -> Result<(), SwarmError>;
}
