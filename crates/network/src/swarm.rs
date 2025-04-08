use libp2p::{Swarm, swarm::NetworkBehaviour};

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
