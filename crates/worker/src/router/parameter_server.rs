use std::path::Path;

use hypha_network::{
    request_response::RequestResponseInterface,
    stream::{StreamSenderInterface, StreamReceiverInterface},
};
use libp2p::PeerId;
use tokio::fs;
use tracing::{debug, info, warn};
use uuid::Uuid;
use futures::{AsyncWriteExt, AsyncReadExt, StreamExt};

use super::{Location, RouterError};
use crate::network::Network;

/// Pull parameters from a parameter server
/// 
/// This will:
/// - Request parameters from the specified peer
/// - Cache them locally for reuse
/// - Return the path to the cached parameters
pub async fn pull_parameters(
    network: &Network,
    peer_id: &str,
    job_id: Uuid,
    version: Option<u64>,
    key: &str,
    cache_dir: &Path,
) -> Result<Location, RouterError> {
    let peer_id = peer_id.parse::<PeerId>()
        .map_err(|e| RouterError::InvalidReference(format!("Invalid peer ID: {}", e)))?;
    
    // Create cache path for this parameter
    let param_dir = cache_dir
        .join("parameters")
        .join(job_id.to_string())
        .join(key);
    
    let param_file = if let Some(version) = version {
        param_dir.join(format!("v{}.bin", version))
    } else {
        param_dir.join("latest.bin")
    };
    
    // Check if already cached
    if param_file.exists() {
        debug!(
            peer = %peer_id,
            job_id = %job_id,
            key = %key,
            version = ?version,
            cached_path = %param_file.display(),
            "Using cached parameters"
        );
        return Ok(Location::new(param_file));
    }
    
    info!(
        peer = %peer_id,
        job_id = %job_id,
        key = %key,
        version = ?version,
        "Pulling parameters from parameter server"
    );
    
    // Create parameter request message
    let request = hypha_messages::Request::ParameterPull(hypha_messages::parameter_pull::Request {
        job_id,
        key: key.to_string(),
        version,
    });
    
    // Request parameters from parameter server
    let response = network.request(peer_id, request).await
        .map_err(|e| RouterError::Network(format!("Failed to request parameters: {}", e)))?;
    
    match response {
        hypha_messages::Response::ParameterPull(param_response) => {
            match param_response {
                hypha_messages::parameter_pull::Response::Success { version, data_stream_id } => {
                    // Receive parameter data via stream
                    let param_data = receive_parameter_stream(network, peer_id, data_stream_id).await?;
                    
                    // Create cache directory
                    fs::create_dir_all(&param_dir).await?;
                    
                    // Write parameters to cache file
                    let versioned_file = param_dir.join(format!("v{}.bin", version));
                    fs::write(&versioned_file, param_data).await?;
                    
                    // Also create/update "latest" symlink/copy
                    if param_file != versioned_file {
                        if let Err(e) = fs::copy(&versioned_file, &param_file).await {
                            warn!(error = ?e, "Failed to create latest parameter copy");
                        }
                    }
                    
                    info!(
                        peer = %peer_id,
                        job_id = %job_id,
                        key = %key,
                        version = version,
                        path = %versioned_file.display(),
                        "Parameters cached successfully"
                    );
                    
                    Ok(Location::new(versioned_file))
                }
                hypha_messages::parameter_pull::Response::NotFound => {
                    Err(RouterError::NotFound(format!("Parameters not found for job {} key {}", job_id, key)))
                }
                hypha_messages::parameter_pull::Response::Error(msg) => {
                    Err(RouterError::Network(format!("Parameter server error: {}", msg)))
                }
            }
        }
        _ => Err(RouterError::Network("Unexpected response type from parameter server".to_string()))
    }
}

/// Push parameters to a parameter server
/// 
/// This will:
/// - Send parameter data to the specified peer
/// - Handle versioning and consistency
pub async fn push_parameters(
    network: &Network,
    peer_id: &str,
    job_id: Uuid,
    version: Option<u64>,
    key: &str,
    location: &Location,
) -> Result<(), RouterError> {
    let peer_id = peer_id.parse::<PeerId>()
        .map_err(|e| RouterError::InvalidReference(format!("Invalid peer ID: {}", e)))?;
    
    info!(
        peer = %peer_id,
        job_id = %job_id,
        key = %key,
        version = ?version,
        path = %location.path.display(),
        "Pushing parameters to parameter server"
    );
    
    // Read parameter data
    let param_data = fs::read(&location.path).await?;
    
    // Send parameter data via stream first
    let stream_id = send_parameter_stream(network, peer_id, &param_data).await?;
    
    // Create parameter push request
    let request = hypha_messages::Request::ParameterPush(hypha_messages::parameter_push::Request {
        job_id,
        key: key.to_string(),
        version,
        data_stream_id: stream_id,
        data_size: param_data.len() as u64,
    });
    
    // Send push request to parameter server
    let response = network.request(peer_id, request).await
        .map_err(|e| RouterError::Network(format!("Failed to push parameters: {}", e)))?;
    
    match response {
        hypha_messages::Response::ParameterPush(param_response) => {
            match param_response {
                hypha_messages::parameter_push::Response::Success { version } => {
                    info!(
                        peer = %peer_id,
                        job_id = %job_id,
                        key = %key,
                        version = version,
                        "Parameters pushed successfully"
                    );
                    Ok(())
                }
                hypha_messages::parameter_push::Response::Error(msg) => {
                    Err(RouterError::Network(format!("Parameter server error: {}", msg)))
                }
            }
        }
        _ => Err(RouterError::Network("Unexpected response type from parameter server".to_string()))
    }
}

/// Send parameter data via libp2p stream
async fn send_parameter_stream(
    network: &Network,
    peer_id: PeerId,
    data: &[u8],
) -> Result<Uuid, RouterError> {
    let stream_id = Uuid::new_v4();
    
    // Open stream to parameter server
    let mut stream = network.stream(peer_id).await
        .map_err(|e| RouterError::Network(format!("Failed to open stream: {}", e)))?;
    
    // Send stream header with ID
    let header = hypha_messages::ParameterStreamHeader {
        stream_id,
        data_size: data.len() as u64,
    };
    
    // Send header and data
    let mut header_bytes = Vec::new();
    ciborium::ser::into_writer(&header, &mut header_bytes)
        .map_err(|e| RouterError::Network(format!("Failed to serialize header: {}", e)))?;
    
    stream.write_all(&header_bytes).await
        .map_err(|e| RouterError::Network(format!("Failed to send header: {}", e)))?;
    
    stream.write_all(data).await
        .map_err(|e| RouterError::Network(format!("Failed to send data: {}", e)))?;
    
    stream.flush().await
        .map_err(|e| RouterError::Network(format!("Failed to flush stream: {}", e)))?;
    
    Ok(stream_id)
}

/// Receive parameter data via libp2p stream
async fn receive_parameter_stream(
    network: &Network,
    peer_id: PeerId,
    stream_id: Uuid,
) -> Result<Vec<u8>, RouterError> {
    use std::time::Duration;
    
    info!(
        peer = %peer_id,
        stream_id = %stream_id,
        "Waiting for parameter stream"
    );
    
    // Wait for incoming stream from the parameter server
    // NOTE: We wait for a reasonable timeout to receive the stream
    let mut incoming_streams = network.streams()
        .map_err(|e| RouterError::Network(format!("Failed to register stream protocol: {}", e)))?;
    
    let (stream_peer_id, mut stream) = tokio::time::timeout(
        Duration::from_secs(30), // 30 second timeout
        incoming_streams.next()
    ).await
    .map_err(|_| RouterError::Network("Timeout waiting for parameter stream".to_string()))?
    .ok_or_else(|| RouterError::Network("Stream channel closed unexpectedly".to_string()))?;
    
    // Verify the stream is from the expected peer
    if stream_peer_id != peer_id {
        return Err(RouterError::Network(format!(
            "Received stream from unexpected peer: expected {}, got {}", 
            peer_id, stream_peer_id
        )));
    }
    
    // Read and deserialize the stream header
    let mut header_buf = Vec::new();
    let mut temp_buf = [0u8; 4096];
    
    // Read header (CBOR-encoded ParameterStreamHeader)
    // NOTE: We read until we can deserialize a valid header
    loop {
        let n = stream.read(&mut temp_buf).await
            .map_err(|e| RouterError::Network(format!("Failed to read stream header: {}", e)))?;
        
        if n == 0 {
            return Err(RouterError::Network("Stream closed before header received".to_string()));
        }
        
        header_buf.extend_from_slice(&temp_buf[..n]);
        
        // Try to deserialize header
        if let Ok(header) = ciborium::de::from_reader::<hypha_messages::ParameterStreamHeader, _>(
            std::io::Cursor::new(&header_buf)
        ) {
            // Verify this is the stream we're expecting
            if header.stream_id != stream_id {
                return Err(RouterError::Network(format!(
                    "Stream ID mismatch: expected {}, got {}", 
                    stream_id, header.stream_id
                )));
            }
            
            info!(
                peer = %peer_id,
                stream_id = %stream_id,
                data_size = header.data_size,
                "Received parameter stream header"
            );
            
            // Calculate how much header data we consumed
            let mut header_bytes = Vec::new();
            ciborium::ser::into_writer(&header, &mut header_bytes)
                .map_err(|e| RouterError::Network(format!("Failed to reserialize header: {}", e)))?;
            
            // The remaining data in header_buf is part of the payload
            let payload_start = header_bytes.len();
            let mut param_data = Vec::with_capacity(header.data_size as usize);
            
            if header_buf.len() > payload_start {
                param_data.extend_from_slice(&header_buf[payload_start..]);
            }
            
            // Read remaining parameter data
            while param_data.len() < header.data_size as usize {
                let remaining = header.data_size as usize - param_data.len();
                let to_read = remaining.min(temp_buf.len());
                
                let n = stream.read(&mut temp_buf[..to_read]).await
                    .map_err(|e| RouterError::Network(format!("Failed to read parameter data: {}", e)))?;
                
                if n == 0 {
                    return Err(RouterError::Network("Stream closed before all data received".to_string()));
                }
                
                param_data.extend_from_slice(&temp_buf[..n]);
            }
            
            info!(
                peer = %peer_id,
                stream_id = %stream_id,
                bytes_received = param_data.len(),
                "Parameter stream received successfully"
            );
            
            return Ok(param_data);
        }
        
        // If header buffer gets too large, something is wrong
        if header_buf.len() > 4096 {
            return Err(RouterError::Network("Stream header too large".to_string()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[tokio::test]
    async fn test_parameter_cache_path_generation() {
        let temp_dir = TempDir::new().unwrap();
        let cache_dir = temp_dir.path();
        
        let job_id = Uuid::new_v4();
        let key = "global_weights";
        
        let param_dir = cache_dir
            .join("parameters")
            .join(job_id.to_string())
            .join(key);
        
        let param_file_v1 = param_dir.join("v1.bin");
        let param_file_latest = param_dir.join("latest.bin");
        
        assert_ne!(param_file_v1, param_file_latest);
        assert_eq!(param_file_v1.file_name().unwrap(), "v1.bin");
        assert_eq!(param_file_latest.file_name().unwrap(), "latest.bin");
    }
}