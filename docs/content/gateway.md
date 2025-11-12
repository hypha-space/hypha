# Gateway

Gateway nodes provide stable entry points for the Hypha network. They enable peer discovery, facilitate NAT traversal, and anchor the distributed hash table. This section covers gateway deployment and configuration.

## Role and Responsibilities

Gateways serve three critical network functions:

**Stable Entry Points**: New nodes joining the network connect to gateways listed in their bootstrap configuration. Gateways maintain well-known, stable addresses that serve as initial connection points.

**Relay Functionality**: Nodes behind NAT or firewalls cannot accept inbound connections directly. Gateways act as relays, forwarding traffic between peers that cannot connect directly. This enables full network participation even for nodes with restricted connectivity.

**DHT Stability**: The Kademlia DHT relies on stable, long-lived nodes to maintain routing table quality. Gateways anchor the DHT by maintaining connections with many peers and advertising consistent peer IDs over time.

Gateways participate in all network protocols (Kademlia, Gossipsub, request/response, streams) but do not execute compute tasks, coordinate training, or store datasets.

## Installation and Setup

Install the gateway binary following the [Installation](installation.md) guide.

Quick reference:

```bash
# From pre-built binary
curl -fsSL https://github.com/hypha-space/hypha/releases/latest/download/install.sh | sh

# Or build from source
cargo build --release -p hypha-gateway
```

### Deployment Considerations

**Public IP Address**: Gateways should have public IP addresses reachable from all nodes. While gateways can run behind NAT using relay-through-relay, this reduces effectiveness.

**Stable Addressing**: Use static IP addresses or stable DNS names. Frequent address changes disrupt network connectivity.

**Firewall Rules**: Ensure inbound connections are allowed on configured listen ports. Both TCP and optionally QUIC (UDP) should be permitted.

**High Availability**: Deploy multiple gateways in different network locations for redundancy. If one gateway fails, nodes can bootstrap through others.

**Resource Requirements**: Gateways require minimal CPU and memory (typically <1GB RAM, <10% CPU). Network bandwidth scales with relay traffic volume.

## Configuration Parameters

Gateway configuration uses TOML format with security and network settings.

### TLS Certificates

Required for mTLS authentication:

```toml
cert_pem = "/etc/hypha/gateway-cert.pem"
key_pem = "/etc/hypha/gateway-key.pem"
trust_pem = "/etc/hypha/ca-bundle.pem"
crls_pem = "/etc/hypha/crls.pem"  # Optional
```

See [Security](security.md) documentation for certificate generation and management.

### Network Addresses

**Listen Addresses**: Local addresses to bind.

```toml
listen_addresses = [
    "/ip4/0.0.0.0/tcp/4001",
    "/ip6/::/tcp/4001",
]
```

Common patterns:
- `/ip4/0.0.0.0/tcp/<port>`: Listen on all IPv4 interfaces
- `/ip6/::/tcp/<port>`: Listen on all IPv6 interfaces
- `/ip4/127.0.0.1/tcp/<port>`: Listen on localhost only (testing)

**External Addresses**: Publicly reachable addresses to advertise.

```toml
external_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/dns4/gateway1.example.com/tcp/4001",
]
```

External addresses must be reachable from all peers. Use DNS names for dynamic IP scenarios or load balancers.

**Address Filtering**: Exclude specific CIDR ranges from peer discovery.

```toml
exclude_cidr = [
    "10.0.0.0/8",      # Private networks
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8",     # Loopback
    "fc00::/7",        # IPv6 ULA
]
```

Defaults to reserved IP ranges (loopback, private networks, link-local).

### OpenTelemetry Configuration

Optional observability integration:

```toml
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"

[telemetry_attributes]
"service.name" = "hypha-gateway"
"deployment.environment" = "production"
"gateway.region" = "us-west-2"

[telemetry_headers]
"Authorization" = "Bearer <token>"
```

See [OpenTelemetry Integration](#opentelemetry-integration) section below for details.

## Example Configuration

Complete production gateway configuration:

```toml
# TLS Configuration
cert_pem = "/etc/hypha/certs/gateway-cert.pem"
key_pem = "/etc/hypha/certs/gateway-key.pem"
trust_pem = "/etc/hypha/certs/ca-bundle.pem"
crls_pem = "/etc/hypha/certs/crls.pem"

# Network Configuration
listen_addresses = [
    "/ip4/0.0.0.0/tcp/4001",
    "/ip6/::/tcp/4001",
]

external_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip6/2001:db8::1/tcp/4001",
]

exclude_cidr = [
    "10.0.0.0/8",
    "172.16.0.0/12",
    "192.168.0.0/16",
    "127.0.0.0/8",
    "fc00::/7",
]

# Telemetry
telemetry_endpoint = "http://otel-collector:4318"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.1

[telemetry_attributes]
"service.name" = "hypha-gateway"
"service.version" = "0.1.0"
"deployment.environment" = "production"
"gateway.location" = "us-west-2a"
```

## CLI Reference

Full command-line reference will be available when [PR #118](https://github.com/hypha-space/hypha/pull/118) merges.

**Basic Usage**:
```bash
hypha-gateway --config /path/to/config.toml
```

**Common Options**:
- `--config <FILE>`: Path to configuration file (required)
- `--help`: Display help information
- `--version`: Show version information

**Environment Variables**:

Override configuration via environment:
```bash
HYPHA_CERT_PEM=/path/to/cert.pem \
HYPHA_KEY_PEM=/path/to/key.pem \
HYPHA_TRUST_PEM=/path/to/ca.pem \
hypha-gateway --config config.toml
```

## Network Protocols

Gateways participate in all libp2p protocols used by Hypha:

### Kademlia DHT

Gateways maintain Kademlia routing tables and respond to DHT queries:

**Provider Records**: When data nodes announce datasets, gateways store provider records enabling schedulers to locate data sources.

**Peer Routing**: Gateways answer peer location queries, helping nodes find each other through the DHT.

**Routing Table Stability**: Long-lived gateway connections improve routing table quality across the network.

### Gossipsub

Gateways participate in Gossipsub mesh networks:

**Topic Subscription**: Gateways subscribe to relevant topics (e.g., `hypha/worker`) to participate in mesh topology.

**Message Forwarding**: Gateways relay Gossipsub messages between peers, improving message propagation reliability.

**Mesh Maintenance**: Gateways contribute to mesh topology health through heartbeat and graft/prune protocols.

### Circuit Relay

The circuit relay protocol enables NAT traversal:

**Relay Reservations**: Nodes behind NAT can reserve relay circuits through gateways, enabling them to receive inbound connections.

**Traffic Forwarding**: Gateways forward relayed connections between peers that cannot connect directly.

**Hop Limits**: Relay circuits are limited to prevent excessive forwarding overhead.

### Request/Response

Gateways respond to direct request/response protocol messages, enabling reliable point-to-point communication for lease renewals, worker offers, and other coordination messages.

## High Availability Configurations

Deploy multiple gateways for redundancy:

### Geographic Distribution

```
Region 1              Region 2              Region 3
Gateway A             Gateway B             Gateway C
203.0.113.50          198.51.100.25         192.0.2.100
```

Nodes configure all gateways in bootstrap lists:

```toml
gateway_addresses = [
    "/ip4/203.0.113.50/tcp/4001",
    "/ip4/198.51.100.25/tcp/4001",
    "/ip4/192.0.2.100/tcp/4001",
]
```

If one gateway fails, nodes bootstrap through others.

### Load Balancing

Use DNS round-robin or load balancers:

```toml
external_addresses = [
    "/dns4/gateway.example.com/tcp/4001",
]
```

DNS resolves to multiple gateway IPs, distributing load. Note that libp2p peer IDs are derived from certificates, so each backend gateway has a distinct peer ID.

### Active/Standby

Run standby gateways with identical configuration. If primary fails, update DNS or reconfigure clients to use standby addresses.

## OpenTelemetry Integration

Gateways export telemetry for monitoring and debugging.

### Endpoint Configuration

```toml
telemetry_endpoint = "http://localhost:4318"
telemetry_protocol = "http/protobuf"  # or "grpc", "http/json"
```

**Protocol Options**:
- `grpc`: gRPC protocol (port 4317 typically)
- `http/protobuf`: HTTP with protobuf encoding (port 4318)
- `http/json`: HTTP with JSON encoding (port 4318)

### Authentication

```toml
[telemetry_headers]
"Authorization" = "Bearer <api-token>"
"X-Scope-OrgID" = "<tenant-id>"
```

### Service Attributes

```toml
[telemetry_attributes]
"service.name" = "hypha-gateway"
"service.version" = "0.1.0"
"deployment.environment" = "production"
"gateway.region" = "us-west-2"
"gateway.instance" = "gw-01"
```

### Gateway-Specific Metrics

Gateways export these metrics:

**Connection Metrics**:
- Active connection count
- Connection establishment rate
- Connection failure rate
- Connections by peer type (scheduler, worker, data node)

**Relay Statistics**:
- Active relay circuits
- Relay bytes transferred
- Relay reservation success/failure
- Circuit timeout events

**DHT Metrics**:
- Routing table size
- Query success rate
- Provider record count
- Peer discovery latency

**Gossipsub Metrics**:
- Mesh peer count per topic
- Message propagation latency
- Duplicate message rate
- Heartbeat cycle times

### Trace Sampling

Control trace volume for high-traffic gateways:

```toml
telemetry_sampler = "traceidratio"
telemetry_sample_ratio = 0.01  # Sample 1% of traces
```

## Troubleshooting

**Nodes cannot connect to gateway**:
- Verify firewall rules allow inbound connections on listen ports
- Check external address is actually reachable (use `telnet` or `nc` to test)
- Confirm certificates are valid and trusted by clients
- Review logs for TLS handshake failures

**Gateway high CPU usage**:
- Monitor relay circuit count (excessive relaying increases CPU)
- Check for Gossipsub message storms
- Consider adding more gateways to distribute load
- Verify no peers are misbehaving (flooding messages)

**DHT queries failing**:
- Confirm gateway maintains healthy routing table (check peer count)
- Verify network connectivity to other DHT participants
- Check for network partitions isolating gateway

**Certificate errors**:
- Ensure certificate, key, and trust bundle paths are correct
- Verify certificate is not expired
- Check certificate is not in CRL
- Confirm trust bundle contains valid CA certificates

**Relay circuits failing**:
- Check relay limits aren't exhausted
- Verify both relay endpoints have valid connectivity
- Review logs for circuit timeout messages
- Confirm NAT traversal is configured correctly

## Next Steps

With gateways deployed:

1. Configure [Schedulers](scheduler.md) to use gateway addresses for bootstrap
2. Set up [Workers](worker.md) to connect through gateways
3. Deploy [Data Nodes](data-node.md) for dataset serving
4. Review [Security](security.md) for production hardening
5. Set up monitoring via [OpenTelemetry](operations.md#monitoring-and-observability)
