# Gateway

Gateway nodes provide stable entry points for the Hypha network. They enable peer discovery, facilitate NAT traversal, and anchor the distributed hash table. This section covers gateway deployment and configuration.

**CLI Reference**: See the [hypha-gateway CLI Reference](../reference/hypha-gateway-cli.md) for complete command-line documentation.

## Role and Responsibilities

Gateways serve three critical network functions:

**Stable Entry Points**: New nodes joining the network connect to gateways listed in their bootstrap configuration. Gateways maintain well-known, stable addresses that serve as initial connection points.

**Relay Functionality**: Nodes behind NAT or firewalls cannot accept inbound connections directly. Gateways act as relays, forwarding traffic between peers that cannot connect directly. This enables full network participation even for nodes with restricted connectivity.

**DHT Stability**: The Kademlia DHT relies on stable, long-lived nodes to maintain routing table quality. Gateways anchor the DHT by maintaining connections with many peers and advertising consistent peer IDs over time.

Gateways participate in all network protocols (Kademlia, Gossipsub, request/response, streams) but do not execute compute tasks, coordinate training, or store datasets.

## Installation

Install the data node binary following the [Installation](installation.md) guide.

## Configuration Parameters

Gateway configuration uses TOML format with security and network settings. Generate an example configuration file using the [`hypha-gateway init`](../reference/hypha-gateway-cli.md#hypha-gateway-init) command. You will need to provide paths to TLS certificates and configure basic network settings (listen addresses, external addresses).

### OpenTelemetry

OpenTelemetry enables distributed tracing and metrics collection for debugging and monitoring your gateways in production. Configure telemetry either via the TOML configuration file or using standard `OTEL_*` environment variables.

**Configuration File (Example uses Grafana Cloud)**: Specify telemetry settings in your `gateway.toml`:

```toml
telemetry_attributes = "service.name=<Gateway Name>,service.namespace=<Namespace>,deployment.environment=<Environment>"
telemetry_endpoint = "https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
telemetry_headers = "Authorization=Basic <Api Key>"
telemetry_protocol = "http/protobuf"
telemetry_sampler = "parentbased_traceidratio"
telemetry_sample_ratio = 0.1
```

**Environment Variables (Example uses Grafana Cloud)**: Alternatively, use standard OpenTelemetry environment variables:

```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="https://otlp-gateway-prod-eu-west-2.grafana.net/otlp"
export OTEL_EXPORTER_OTLP_HEADERS="Authorization=Basic <Api Key>"
export OTEL_EXPORTER_OTLP_PROTOCOL="http/protobuf"
export OTEL_SERVICE_NAME="gateway-01"
export OTEL_RESOURCE_ATTRIBUTES="service.namespace=production,deployment.environment=prod"

hypha-gateway run --config /etc/hypha/gateway.toml
```

Environment variables take precedence over configuration file settings, allowing flexible per-deployment customization.

## Network Protocols

Gateways participate in all libp2p protocols used by Hypha:

### Kademlia DHT

Gateways maintain Kademlia routing tables and respond to DHT queries. When data nodes announce datasets, gateways store provider records that enable schedulers to locate data sources. They also answer peer location queries, helping nodes discover each other through the DHT. Long-lived gateway connections improve routing table quality across the network, making peer discovery more reliable and efficient.

### Gossipsub

Gateways participate in Gossipsub mesh networks by subscribing to relevant topics (such as `hypha/worker`) and relaying messages between peers. This improves message propagation reliability across the network. Gateways contribute to mesh topology health through heartbeat and graft/prune protocols, ensuring efficient message delivery and maintaining optimal mesh connectivity.

### Circuit Relay

The circuit relay protocol enables NAT traversal by allowing nodes behind firewalls to reserve relay circuits through gateways. Gateways forward relayed connections between peers that cannot connect directly, enabling full network participation regardless of network restrictions. Relay circuits are limited by hop count to prevent excessive forwarding overhead and maintain network efficiency.
