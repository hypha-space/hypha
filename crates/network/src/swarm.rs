//! Trait wrappers around the libp2p `Swarm`.
//!
//! Provides a minimal interface for running the swarm and retrieving mutable
//! access to it. Higher level components implement these traits to drive the
//! networking event loop.

use libp2p::{Swarm, swarm::NetworkBehaviour};
use thiserror::Error;

/// Errors that can occur during swarm operations and configuration.
#[derive(Debug, Error)]
pub enum SwarmError {
    /// Transport layer configuration failed.
    #[error("Transport configuration failed: {0}")]
    TransportConfig(String),
    /// Network behaviour instantiation failed.
    #[error("Behaviour creation failed: {0}")]
    BehaviourCreation(String),
    /// Swarm-level configuration failed.
    #[error("Swarm configuration failed: {0}")]
    SwarmConfig(String),
}

/// Base trait for managing a libp2p swarm and driving network operations.
///
/// This trait provides the foundational interface for all network drivers in the
/// Hypha network stack. It gives access to the underlying libp2p swarm and defines
/// the main event loop for processing network events.
#[allow(async_fn_in_trait)]
pub trait SwarmDriver<TBehavior>
where
    TBehavior: NetworkBehaviour,
    Self: Sized,
{
    /// Returns a mutable reference to the underlying libp2p swarm.
    ///
    /// This provides direct access to the swarm for advanced operations
    /// that are not covered by the higher-level trait methods.
    fn swarm(&mut self) -> &mut Swarm<TBehavior>;

    /// Run the swarm driver, processing incoming actions and managing the swarm.
    ///
    /// This is the main event loop that should handle all network events,
    /// action processing, and swarm management. The implementation should
    /// run indefinitely until an error occurs or shutdown is requested.
    ///
    /// # Errors
    ///
    /// Returns [`SwarmError`] if a critical network error occurs that
    /// prevents continued operation.
    async fn run(self) -> Result<(), SwarmError>;
}
