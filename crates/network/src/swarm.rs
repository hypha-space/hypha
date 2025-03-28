use libp2p::Swarm;
use libp2p::identity::Keypair;
use libp2p::swarm::NetworkBehaviour;

use crate::error::HyphaError;

#[allow(async_fn_in_trait)]
pub trait SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
    Self: Sized,
{
    fn swarm(&mut self) -> &mut Swarm<TBehavior>;

    /// Run the swarm driver, processing incoming actions and managing the swarm.
    async fn run(self) -> Result<(), HyphaError>;
}

#[allow(async_fn_in_trait)]
pub trait SwarmInterface<TBehavior>
where
    TBehavior: NetworkBehaviour,
    Self: Sized,
{
    type Driver: SwarmDriver<TBehavior>;

    /// Create a new swarm instance with the given identity.
    fn create(identity: Keypair) -> Result<(Self, Self::Driver), HyphaError>;
}
