use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub mod cache;
pub mod resolver;
pub mod parameter_server;

// Re-export from hypha_messages for convenience
pub use hypha_messages::Reference;

/// Represents a local filesystem location
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Location {
    /// Path to the file or directory on local filesystem
    pub path: PathBuf,
}

impl Location {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
    
    pub fn from_str(path: &str) -> Self {
        Self { path: PathBuf::from(path) }
    }
}

/// Stream receiver for accepting tensor streams with peer validation
/// 
/// This represents an active receive window that validates incoming
/// streams and provides event notifications when data arrives.
#[derive(Debug)]
pub struct StreamReceiver {
    /// Peer ID that is allowed to send streams to us
    pub allowed_sender: String,
    /// Job ID for stream isolation
    pub job_id: Uuid,
    /// Stream kind (e.g., "parameters", "gradients", "checkpoints")
    pub stream_kind: String,
    /// Cache directory for storing received streams
    pub cache_dir: PathBuf,
}

impl StreamReceiver {
    /// Wait for the next stream from the allowed sender
    /// Returns the location where the stream data was stored
    pub async fn next_stream(&mut self) -> Result<Location, RouterError> {
        // TODO: Implement actual stream waiting with peer validation
        // This should:
        // 1. Listen for incoming streams
        // 2. Validate sender peer ID matches allowed_sender
        // 3. Validate job_id and stream_kind
        // 4. Store stream data to cache_dir
        // 5. Return Location of stored data
        
        Err(RouterError::UnsupportedOperation("StreamReceiver.next_stream() not yet implemented".to_string()))
    }
}

/// Router for content-addressable storage and parameter server communication
/// 
/// The router resolves References (URIs, HuggingFace models, CIDs, Parameter Servers) 
/// to local Locations and enables pushing results back to storage or parameter servers.
#[derive(Clone)]
pub struct Router {
    /// Base directory for caching pulled content
    cache_dir: PathBuf,
    /// Base directory for storing pushed results  
    output_dir: PathBuf,
    /// Network interface for parameter server communication
    network: Option<crate::network::Network>,
}

impl Router {
    pub fn new(cache_dir: impl Into<PathBuf>, output_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            output_dir: output_dir.into(),
            network: None,
        }
    }
    
    pub fn with_network(mut self, network: crate::network::Network) -> Self {
        self.network = Some(network);
        self
    }
    
    /// Pull content from a Reference to a local Location
    /// 
    /// This will:
    /// - Download remote content if not cached
    /// - Return the local path where content is available
    /// - Handle different reference types (URI, HuggingFace, Parameter Server, future CIDs)
    pub async fn pull(&self, reference: &Reference) -> Result<Location, RouterError> {
        match reference {
            Reference::Uri { value } => {
                resolver::pull_uri(value, &self.cache_dir).await
            }
            Reference::HuggingFace { repository, revision, filenames } => {
                resolver::pull_huggingface(repository, revision.as_deref(), filenames, &self.cache_dir).await
            }
            Reference::ParameterServer { peer_id, job_id, version, key } => {
                let network = self.network.as_ref().ok_or_else(|| RouterError::UnsupportedOperation("Network not configured for parameter server access".to_string()))?;
                parameter_server::pull_parameters(
                    network,
                    peer_id,
                    *job_id,
                    *version,
                    key,
                    &self.cache_dir
                ).await
            }
            Reference::StreamReceiver { .. } => {
                Err(RouterError::UnsupportedOperation("StreamReceiver references cannot be pulled - use receive_stream() instead".to_string()))
            }
            Reference::StreamSender { .. } => {
                Err(RouterError::UnsupportedOperation("StreamSender references cannot be pulled - use send_stream() instead".to_string()))
            }
        }
    }
    
    /// Push content from a local Location to a Reference
    /// 
    /// This will:
    /// - Upload local content to the specified reference
    /// - Handle different storage backends based on reference type
    /// - Send parameters to parameter servers for distributed training
    pub async fn push(&self, reference: &Reference, location: &Location) -> Result<(), RouterError> {
        match reference {
            Reference::Uri { value } => {
                resolver::push_uri(value, location, &self.output_dir).await
            }
            Reference::HuggingFace { .. } => {
                // NOTE: Pushing to HuggingFace requires authentication
                // For now, we don't support this
                Err(RouterError::UnsupportedOperation("Cannot push to HuggingFace".to_string()))
            }
            Reference::ParameterServer { peer_id, job_id, version, key } => {
                let network = self.network.as_ref().ok_or_else(|| RouterError::UnsupportedOperation("Network not configured for parameter server access".to_string()))?;
                parameter_server::push_parameters(
                    network,
                    peer_id,
                    *job_id,
                    *version,
                    key,
                    location
                ).await
            }
            Reference::StreamReceiver { .. } => {
                Err(RouterError::UnsupportedOperation("Cannot push to StreamReceiver reference - use send_stream() instead".to_string()))
            }
            Reference::StreamSender { target_peer, job_id, stream_kind, .. } => {
                // Use existing parameter server infrastructure for now
                let network = self.network.as_ref().ok_or_else(|| RouterError::UnsupportedOperation("Network not configured for stream sending".to_string()))?;
                parameter_server::push_parameters(
                    network,
                    target_peer,
                    *job_id,
                    None, // No versioning for streams initially
                    stream_kind,
                    location
                ).await
            }
        }
    }
    
    /// Start receiving tensor streams from allowed peers
    /// 
    /// This implements the "Receive" verb from the design - declares readiness
    /// to accept streams during a defined time window with peer validation.
    pub async fn receive_stream(
        &self, 
        stream_ref: &Reference
    ) -> Result<StreamReceiver, RouterError> {
        match stream_ref {
            Reference::StreamReceiver { allowed_sender, job_id, stream_kind, metadata: _ } => {
                let _network = self.network.as_ref().ok_or_else(|| {
                    RouterError::UnsupportedOperation("Network not configured for stream receiving".to_string())
                })?;
                
                // TODO: Implement actual stream receiver with peer validation
                // For now, return a placeholder
                Ok(StreamReceiver {
                    allowed_sender: allowed_sender.clone(),
                    job_id: *job_id,
                    stream_kind: stream_kind.clone(),
                    cache_dir: self.cache_dir.clone(),
                })
            }
            _ => Err(RouterError::InvalidReference("Expected StreamReceiver reference".to_string()))
        }
    }
    
    /// Send tensor stream to a specific peer
    /// 
    /// This implements the "Send" verb from the design - actively pushes
    /// tensor data to a known peer.
    pub async fn send_stream(
        &self,
        stream_ref: &Reference,
        location: &Location
    ) -> Result<(), RouterError> {
        match stream_ref {
            Reference::StreamSender { target_peer, job_id, stream_kind, .. } => {
                let network = self.network.as_ref().ok_or_else(|| {
                    RouterError::UnsupportedOperation("Network not configured for stream sending".to_string())
                })?;
                
                // Reuse parameter server infrastructure for sending streams
                parameter_server::push_parameters(
                    network,
                    target_peer,
                    *job_id,
                    None, // No versioning for streams initially
                    stream_kind,
                    location
                ).await
            }
            _ => Err(RouterError::InvalidReference("Expected StreamSender reference".to_string()))
        }
    }
}

/// Router-specific errors
#[derive(Debug, thiserror::Error)]
pub enum RouterError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Network error: {0}")]
    Network(String),
    
    #[error("Reference not found: {0}")]
    NotFound(String),
    
    #[error("Invalid reference: {0}")]
    InvalidReference(String),
    
    #[error("Cache error: {0}")]
    Cache(String),
    
    #[error("Unsupported operation: {0}")]
    UnsupportedOperation(String),
}