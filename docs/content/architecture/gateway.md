+++
title = "Gateway Nodes"
description = "Stable network entry points and relay services"
weight = 2
+++

# Gateway Nodes

Gateway nodes serve as the stable entry points to the Hypha network, providing bootstrap, relay, and connectivity services for other peers.

## Overview

Gateway nodes are long-running, publicly accessible nodes that:

- **Bootstrap new peers**: Provide initial network entry points
- **Relay connections**: Enable NAT traversal for restricted peers
- **Maintain stability**: Serve as stable, well-known addresses
- **Bridge networks**: Connect isolated network segments

## Responsibilities

### 1. Bootstrap Service

New peers joining the network:
1. Connect to one or more gateway nodes
2. Receive initial peer addresses from gateways
3. Join the Kademlia DHT via gateway introductions

### 2. Relay Service

For peers behind NAT or firewalls:
- Accept relay reservation requests
- Forward traffic between unreachable peers
- Facilitate hole punching for direct connections
- Limit relay circuits to prevent abuse

### 3. Network Health

Gateways contribute to network stability:
- Participate actively in the DHT
- Maintain connections to diverse peers
- Provide redundant network paths
- Monitor overall network health

## Architecture

```
                    Internet
                       │
                       │
            ┌──────────▼──────────┐
            │   Gateway Node      │
            │  ┌──────────────┐   │
            │  │ Relay Server │   │
            │  └──────────────┘   │
            │  ┌──────────────┐   │
            │  │ DHT Server   │   │
            │  └──────────────┘   │
            └─────────┬───────────┘
                      │
        ┌─────────────┼─────────────┐
        │             │             │
  ┌─────▼────┐  ┌────▼─────┐  ┌───▼──────┐
  │ Worker   │  │ Scheduler│  │  Worker  │
  │  (NAT)   │  │          │  │   (NAT)  │
  └──────────┘  └──────────┘  └──────────┘
```

## Configuration

### Basic Gateway Configuration

```toml
[network]
# Listen on public addresses
listen_addresses = [
    "/ip4/0.0.0.0/tcp/4001/quic-v1",
    "/ip6/::/tcp/4001/quic-v1"
]

# Gateway-specific settings
[gateway]
# Enable relay functionality
relay_enabled = true

# Maximum concurrent relay circuits
max_relay_circuits = 100

# Relay circuit duration limit (seconds)
relay_circuit_duration = 300

# DHT mode (server for gateways)
dht_mode = "server"

# Connection limits
max_connections = 500
max_connections_per_peer = 5
```

### Production Hardening

```toml
[gateway.security]
# Rate limiting
max_relay_requests_per_minute = 10
max_relay_data_per_minute = "100MB"

# Circuit restrictions
deny_private_ips = true
allowed_protocols = ["quic", "tcp"]

# Resource limits
max_bandwidth = "1GB/s"
max_memory = "4GB"

[gateway.monitoring]
# Metrics endpoint
metrics_enabled = true
metrics_port = 9090

# Logging
log_level = "info"
log_format = "json"
```

## Deployment

### Single Gateway

For testing or small deployments:

```bash
# Run a gateway node
./hypha-gateway --config gateway-config.toml
```

### High Availability

For production, deploy multiple gateways:

```yaml
# Example: Kubernetes deployment
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: hypha-gateway
spec:
  replicas: 3
  serviceName: hypha-gateway
  template:
    spec:
      containers:
      - name: gateway
        image: hypha/gateway:latest
        ports:
        - containerPort: 4001
          name: p2p
        - containerPort: 9090
          name: metrics
```

### DNS Configuration

Configure DNS for easy discovery:

```
gateway-1.hypha.example.com  →  203.0.113.1
gateway-2.hypha.example.com  →  203.0.113.2
gateway-3.hypha.example.com  →  203.0.113.3
```

## Resource Requirements

### Minimum Requirements

- **CPU**: 2 cores
- **Memory**: 2 GB RAM
- **Network**: 100 Mbps, public IP
- **Storage**: 10 GB for logs and state

### Recommended for Production

- **CPU**: 4+ cores
- **Memory**: 8 GB RAM
- **Network**: 1 Gbps, multiple IPs
- **Storage**: 50 GB SSD

## Monitoring

### Key Metrics

Monitor these metrics for gateway health:

- **Active connections**: Current peer connections
- **Relay circuits**: Active relay connections
- **Bandwidth**: Ingress/egress throughput
- **DHT size**: Routing table entries
- **Uptime**: Gateway availability

### Example Prometheus Metrics

```
# Active peer connections
hypha_gateway_connections{type="inbound"} 145
hypha_gateway_connections{type="outbound"} 23

# Relay circuits
hypha_gateway_relay_circuits 12
hypha_gateway_relay_bytes_total 1.5e9

# DHT metrics
hypha_gateway_dht_peers 342
hypha_gateway_dht_queries_total 1523
```

## Security Considerations

### Attack Mitigation

Gateways are exposed to:

1. **DDoS attacks**: Rate limiting and connection limits
2. **Resource exhaustion**: Circuit limits and timeouts
3. **Amplification attacks**: Validate relay requests
4. **Sybil attacks**: Reputation systems (future)

### Best Practices

- **Firewall rules**: Restrict management ports
- **TLS certificates**: Use valid, CA-signed certificates
- **Monitoring**: Alert on anomalous traffic
- **Updates**: Keep software up to date
- **Backups**: Regular state backups

## Troubleshooting

### Common Issues

**Problem**: Gateway unreachable from outside

**Solution**: Verify:
- Public IP is accessible
- Firewall allows ports 4001 (or configured port)
- No NAT blocking inbound connections

**Problem**: High relay circuit count

**Solution**:
- Increase `max_relay_circuits` if needed
- Check for misbehaving peers
- Reduce `relay_circuit_duration`

**Problem**: High memory usage

**Solution**:
- Reduce `max_connections`
- Check for connection leaks
- Review DHT configuration

## Next Steps

- [Network Layer](/architecture/network/) - Underlying P2P infrastructure
- [Scheduler Nodes](/architecture/scheduler/) - Task coordination
- [Deployment Guide](/deployment/) - Production deployment
