#![deny(missing_docs)]
//! Networking primitives used across Hypha.
//!
//! The `hypha-network` crate provides a lightweight abstraction over the
//! [`libp2p`](https://libp2p.io) stack so that higher level components can
//! focus on their business logic. It defines traits and helper utilities for
//! common behaviours such as dialing, listening, gossipsub messaging,
//! Kademlia DHT operations, and custom stream protocols.
//!
//! # Features
//!
//! ## Core Components
//!
//! - **[`swarm`]**: Base trait for managing libp2p swarm operations
//! - **[`dial`]**: Outbound connection management and peer dialing
//! - **[`listen`]**: Inbound connection handling and listening
//! - **[`gossipsub`]**: Publish-subscribe messaging over gossipsub protocol
//! - **[`kad`]**: Kademlia DHT operations for peer discovery and routing
//! - **[`request_response`]**: Request-response messaging between peers
//! - **[`stream`]**: Custom streaming protocols for data transfer
//! - **[`cert`]**: Certificate handling
//! - **[`mtls`]**: mTLS functionality for libp2p
//! - **[`utils`]**: Utility functions and common operations
//!
//! # Integration Guide
//!
//! To integrate the network crate into your application:
//!
//! 1. Define your custom `NetworkBehaviour` that implements the required traits
//! 2. Use the driver traits ([`swarm::SwarmDriver`], [`dial::DialDriver`], etc.) in your event loop
//! 3. Expose interface traits ([`dial::DialInterface`], [`listen::ListenInterface`], etc.) to application logic
//! 4. Handle network events asynchronously using the provided action types
//!
//! Each module contains detailed examples and usage patterns for specific
//! networking functionality.

pub mod cert;
pub mod dial;
pub mod external_address;
pub mod gossipsub;
pub mod kad;
pub mod listen;
pub mod request_response;
pub mod stream_pull;
pub mod stream_push;
pub mod swarm;
pub mod utils;

use std::str::FromStr;

// Re-export CIDR net types
pub use ipnet::{IpNet, Ipv4Net, Ipv6Net};
// Re-export commonly used certificate types
pub use rustls::pki_types::{CertificateDer, CertificateRevocationListDer, PrivateKeyDer};
pub use utils::find_containing_cidr;

/// Loopback CIDRs.
pub fn loopback_cidrs() -> Vec<IpNet> {
    vec![
        IpNet::from_str("127.0.0.0/8").expect("valid CIDR"),
        IpNet::from_str("::1/128").expect("valid CIDR"),
    ]
}

/// Private IPv4 CIDRs (RFC1918).
pub fn private_cidrs() -> Vec<IpNet> {
    vec![
        IpNet::from_str("10.0.0.0/8").expect("valid CIDR"),
        IpNet::from_str("172.16.0.0/12").expect("valid CIDR"),
        IpNet::from_str("192.168.0.0/16").expect("valid CIDR"),
    ]
}

/// link-local CIDRs.
pub fn link_local_cidrs() -> Vec<IpNet> {
    vec![
        IpNet::from_str("169.254.0.0/16").expect("valid CIDR"),
        IpNet::from_str("fe80::/10").expect("valid CIDR"),
    ]
}

/// IPv6 unique local address CIDRs.
pub fn unique_local_ipv6_cidrs() -> Vec<IpNet> {
    vec![IpNet::from_str("fc00::/7").expect("valid CIDR")]
}

/// Rreserved CIDRs that should be filtered in most deployments.
pub fn reserved_cidrs() -> Vec<IpNet> {
    [
        loopback_cidrs(),
        private_cidrs(),
        link_local_cidrs(),
        unique_local_ipv6_cidrs(),
    ]
    .into_iter()
    .flatten()
    .collect()
}
