+++
title = "Network Layer"
description = "P2P networking foundation using libp2p"
weight = 1
+++

# Network Layer

The Hypha network layer provides the foundational peer-to-peer (P2P) networking capabilities built on [libp2p](https://libp2p.io/), enabling decentralized communication between all nodes in the system.

## Overview

The network layer (`hypha-network` crate) implements:

- **Transport protocols**: QUIC, TCP with TLS
- **Peer discovery**: Kademlia DHT, mDNS
- **NAT traversal**: Relay, DCUtR, hole punching
- **Messaging**: Request/response, publish/subscribe
- **Security**: mTLS with certificate-based authentication

## Key Components

### Transport Layer

Hypha supports multiple transport protocols:

#### QUIC Transport
- Modern UDP-based transport
- Built-in encryption via TLS 1.3
- Multiplexing and 0-RTT connection establishment
- Primary transport for inter-node communication

#### TCP + TLS Transport
- Traditional TCP with TLS encryption
- Fallback for networks blocking UDP
- Stream multiplexing via Yamux

### Peer Discovery

#### Kademlia DHT

The Distributed Hash Table (DHT) enables:
- Decentralized peer routing and discovery
- Content-addressable storage for peer records
- Efficient lookups with O(log N) complexity

```rust
// Example: Querying the DHT for peers
let query_id = swarm.behaviour_mut()
    .kademlia
    .get_closest_peers(peer_id);
```

#### mDNS (Local Discovery)

For local network environments:
- Automatic peer discovery on LAN
- Zero-configuration networking
- Fast bootstrap without external peers

### Connection Management

The network layer manages:

- **Connection pooling**: Reuse connections efficiently
- **Backpressure handling**: Flow control for slow peers
- **Timeout management**: Detect and close stale connections
- **Bandwidth limits**: Optional rate limiting

### Security

#### mTLS Authentication

All connections use mutual TLS:
- Each peer has a unique certificate
- Certificate-based peer identity
- Prevents unauthorized connections

```rust
// Peer identity derived from certificate
let peer_id = PeerId::from_certificate(&cert);
```

#### Certificate Management

- Self-signed certificates for development
- CA-signed certificates for production
- Certificate rotation support
- Revocation checking (optional)

## Network Protocols

### Identify Protocol

Peers exchange information about:
- Protocol versions
- Supported transports
- Listen addresses
- Agent version

### Ping Protocol

Heartbeat mechanism for:
- Liveness checking
- Latency measurement
- Keep-alive for long-lived connections

### Request/Response

Custom protocols for:
- Task assignment messages
- Status queries
- Control plane operations

### GossipSub

Publish/subscribe messaging for:
- Task announcements
- Status updates
- System-wide notifications

## NAT Traversal

### Circuit Relay

Gateway nodes act as relays:
- Enable communication through NAT
- Temporary relay until direct connection
- Fallback for restrictive firewalls

### DCUtR (Direct Connection Upgrade)

Hole punching to establish direct connections:
1. Initial connection via relay
2. Coordinate hole punching attempt
3. Upgrade to direct connection
4. Close relay connection

## Network Topology

```
┌─────────────┐
│   Gateway   │◄──────┐
│   (Relay)   │       │
└─────────────┘       │
       │              │
       │         ┌────▼────┐
       │         │ Worker  │
       │         │  (NAT)  │
       │         └─────────┘
       │
   ┌───▼────┐
   │Scheduler│
   └────────┘
```

## Configuration

### Basic Network Configuration

```toml
[network]
# Listen on any IPv4 address, random port
listen_addresses = ["/ip4/0.0.0.0/tcp/0/quic-v1"]

# Bootstrap peers for initial discovery
bootstrap_peers = [
    "/ip4/203.0.113.1/tcp/4001/p2p/12D3KooW..."
]

# Enable local network discovery
mdns_enabled = true

# Connection limits
max_connections = 100
```

### Advanced Configuration

```toml
[network.transport]
# QUIC configuration
quic_enabled = true
quic_idle_timeout = 30

# TCP fallback
tcp_enabled = true

[network.dht]
# Kademlia configuration
kademlia_mode = "server"  # or "client"
replication_factor = 20

[network.relay]
# Relay configuration (for gateways)
relay_enabled = true
max_circuits = 50
```

## Usage Examples

### Creating a Network Swarm

```rust
use hypha_network::{Behaviour, Config};
use libp2p::Swarm;

// Load configuration
let config = Config::from_file("config.toml")?;

// Create swarm
let mut swarm = Swarm::new(
    transport,
    Behaviour::new(&config),
    local_peer_id,
)?;

// Start listening
for addr in &config.listen_addresses {
    swarm.listen_on(addr.clone())?;
}
```

### Handling Network Events

```rust
loop {
    match swarm.next().await {
        Some(event) => match event {
            SwarmEvent::NewListenAddr { address, .. } => {
                println!("Listening on {}", address);
            }
            SwarmEvent::ConnectionEstablished { peer_id, .. } => {
                println!("Connected to {}", peer_id);
            }
            // Handle other events...
        },
        None => break,
    }
}
```

## Performance Considerations

- **Connection pooling**: Minimize connection overhead
- **Message batching**: Reduce syscall overhead
- **Zero-copy operations**: Use efficient buffer management
- **Backpressure**: Handle slow consumers gracefully

## Monitoring

Network metrics exposed:
- Active connections count
- Bandwidth usage (ingress/egress)
- DHT routing table size
- Protocol handler latencies

## Next Steps

- [Gateway Nodes](/architecture/gateway/) - Network entry points
- [Scheduler Nodes](/architecture/scheduler/) - Task coordination
- [Worker Nodes](/architecture/worker/) - Task execution
