# Architecture Overview

Hypha's architecture combines decentralized networking with coordinated resource allocation to enable distributed machine learning at scale. This section explains how components interact and the protocols that connect them.

## System Diagram

Hypha consists of four primary node types connected through a peer-to-peer network:

```
                    ┌─────────────────────────────────────┐
                    │         Gateway Nodes               │
                    │  (Network entry & relay)            │
                    └──────────┬──────────────────────────┘
                               │
                    ┌──────────▼──────────────────────────┐
                    │     libp2p Network Layer            │
                    │  - Kademlia DHT                     │
                    │  - Gossipsub pub/sub                │
                    │  - Request/Response                 │
                    │  - Stream push/pull                 │
                    └─┬────────────┬────────────┬─────────┘
                      │            │            │
          ┌───────────▼──┐    ┌───▼─────────┐  │
          │  Scheduler   │    │   Worker    │  │
          │    Nodes     │◄──►│   Nodes     │  │
          └───────┬──────┘    └──┬──────────┘  │
                  │              │              │
                  │              │              │
          ┌───────▼──────────────▼──────────────▼──┐
          │         Data Nodes                     │
          │    (Dataset storage & serving)         │
          └────────────────────────────────────────┘
```

Data flows through the system as follows:

1. **Discovery**: Schedulers and workers discover each other through Gossipsub topic subscriptions
2. **Negotiation**: Schedulers publish task requirements; workers respond with offers
3. **Data Loading**: Workers fetch dataset slices from data nodes via direct streams
4. **Training**: Workers execute local training steps and report metrics to schedulers
5. **Synchronization**: Workers send pseudo-gradients to a parameter server (aggregate executor)
6. **Update**: Parameter server broadcasts updated model weights back to workers

## Major Components and Interactions

### Gateway Nodes

Gateway nodes provide stable entry points for the network. They serve three key functions:

**Network Bootstrap**: New nodes joining the network connect to gateways to discover other peers. Gateways maintain connections to well-known addresses listed in bootstrap configurations.

**NAT Traversal**: Nodes behind firewalls or NAT can use gateways as relays. The gateway forwards traffic between peers that cannot connect directly.

**DHT Stability**: By maintaining long-lived connections and stable addresses, gateways anchor the Kademlia DHT and improve routing table quality across the network.

Gateways participate in all network protocols (Kademlia, Gossipsub, request/response) but do not execute compute tasks or store data. They require minimal resources but benefit from public IP addresses and reliable connectivity.

### Scheduler Nodes

Schedulers orchestrate distributed training jobs. Their responsibilities include:

**Task Advertisement**: Publishing task requirements to workers via Gossipsub on the `hypha/worker` topic. These advertisements specify resource needs (GPU, CPU, memory), executor types, and pricing.

**Worker Selection**: Collecting offers from workers and selecting the best matches based on price, capabilities, and availability. The scheduler implements greedy selection (lowest price first) for straightforward resource allocation.

**Lease Management**: Maintaining resource reservations through periodic lease renewals. Leases prevent workers from accepting competing tasks while ensuring failed schedulers release resources automatically.

**DiLoCo Coordination**: Tracking worker progress and determining optimal synchronization points. The scheduler simulates future completion times based on reported batch processing speeds to minimize stragglers.

**Data Distribution**: Assigning dataset slices to workers and tracking which data has been processed. Slices transition through states (AVAILABLE → ASSIGNED → USED) to ensure complete dataset coverage.

**Metrics Collection**: Aggregating training metrics and optionally forwarding them to AIM for visualization.

Schedulers maintain state for active jobs but can recover from failures through lease expiration mechanisms. For long-running jobs, schedulers should persist worker pool state to enable graceful failover.

### Worker Nodes

Workers execute training and inference tasks. Each worker implements several subsystems:

**Arbiter**: Subscribes to task advertisements and evaluates incoming requests. The arbiter maintains a configurable pricing threshold and only responds to requests meeting resource requirements and price expectations.

**Request Evaluator**: Scores competing requests when multiple schedulers bid for resources simultaneously. Evaluation strategies can prioritize profit maximization, utilization optimization, or affinity-based selection.

**Lease Manager**: Manages time-bounded resource reservations. When a worker sends an offer, it creates a temporary lease preventing double-booking. Accepted offers transition to renewable leases maintained through periodic renewal.

**Job Manager**: Executes confirmed jobs with proper isolation. The job manager spawns executor processes, provides them with work directories and communication sockets, and monitors execution.

**Executors**: Handle actual computation. Process executors run training scripts as subprocesses. Aggregate executors implement parameter server functionality for DiLoCo synchronization. See the [Worker Node](worker.md) documentation for executor details.

**Job Bridge**: Provides a language-agnostic HTTP API over Unix sockets, allowing executors to fetch resources, send data to peers, and receive streams without understanding P2P protocols.

Workers can run multiple executors simultaneously, enabling diverse workload support on a single machine.

### Data Nodes

Data nodes store prepared datasets and serve slices to workers on demand:

**Dataset Announcement**: Publishing available datasets to the Kademlia DHT. Schedulers query the DHT to locate data nodes serving required datasets.

**Slice Serving**: Streaming SafeTensors files containing dataset batches to workers. Data nodes support parallel serving to multiple workers.

**SafeTensors Format**: Datasets are stored as SafeTensors files, which provide safe tensor deserialization, memory mapping support, and efficient streaming. While designed for model parameters, SafeTensors works well for preprocessed training data.

Data nodes require storage proportional to dataset size and network bandwidth for serving. They do not participate in training coordination.

### Parameter Server

The parameter server is not a separate node type but rather an aggregate executor running on worker nodes. Its role in DiLoCo training:

**Gradient Collection**: Receiving pseudo-gradients (weight deltas) from workers after their inner optimization loops complete.

**Averaging**: Computing mean gradients across all workers to produce the outer optimization update.

**Nesterov Momentum**: Applying the outer optimizer (Nesterov momentum) to the averaged gradients.

**Weight Broadcasting**: Sending updated model weights back to all workers to begin the next inner loop.

The scheduler coordinates parameter server lifecycle, informing it which workers will participate and setting timeout parameters for gradient collection.

## Network Layer (hypha-network)

Hypha's networking is built on libp2p, leveraging its battle-tested protocols:

### Core Protocols

**Kademlia DHT**
Distributed hash table for peer discovery and content routing. Nodes can:
- Find peers by ID through iterative queries
- Locate providers of specific content (datasets, models)
- Maintain routing tables for efficient lookups

Kademlia enables decentralized discovery without central registries.

**Gossipsub**
Publish/subscribe messaging for topic-based communication. Used for:
- Task advertisements from schedulers to workers
- Coordination messages
- Heartbeats and status updates

Gossipsub provides efficient multicast with configurable reliability through message replay and redundancy.

**Request/Response**
Direct peer-to-peer request/response protocol for:
- Worker offers in response to task advertisements
- Lease renewal negotiations
- Status queries

Request/response complements Gossipsub by providing reliable, acknowledged communication for critical messages.

**Stream Push/Pull**
High-throughput streaming for large data transfers:
- Dataset slices from data nodes to workers
- Model weights and gradients between workers and parameter servers
- Arbitrary resource transfers via the job bridge

Streams use TCP with yamux multiplexing, achieving approximately 1 GB/s throughput through parallel asynchronous transfers.

### Transport Layer

**TCP**: Primary transport with TLS 1.3 for encryption and mTLS for authentication. TCP provides reliable, ordered delivery with automatic retransmission.

**QUIC**: Supported by libp2p but currently requires standard libp2p certificates (not compatible with CA-signed certificates in Hypha's mTLS implementation). Future work may enable QUIC with custom certificate validation.

All network communication is encrypted. Hypha does not support unencrypted transports.

### Performance Characteristics

The network layer is optimized for diverse deployment scenarios:

- **Throughput**: Up to 1 GB/s for stream transfers on local networks
- **Discovery**: Peer discovery typically completes in under 1 second
- **Reliability**: Automatic reconnection and message replay handle transient failures
- **Scalability**: Kademlia DHT scales logarithmically; Gossipsub supports thousands of peers per topic

## Deployment Patterns

Hypha adapts to different infrastructure configurations:

### Single-Datacenter

All nodes within one network location:

```
Datacenter
├── Gateway (1-2 nodes)
├── Scheduler (1+ nodes)
├── Workers (N nodes)
└── Data Nodes (1+ nodes)
```

Benefits:
- Low latency between all components
- High bandwidth for data transfers
- Simplified network configuration

Best for: Organizations consolidating existing GPU resources in one location.

### Multi-Region Distributed Training

Nodes spread across geographic regions:

```
Region A                Region B                Region C
├── Gateway             ├── Gateway             ├── Gateway
├── Workers (N)         ├── Workers (M)         ├── Workers (K)
└── Data Node           └── Data Node           └── Data Node

        Scheduler (any region)
```

Benefits:
- Utilizes geographically dispersed resources
- DiLoCo minimizes cross-region communication
- Regional data nodes reduce WAN transfers

Challenges:
- Higher latency between regions
- Network partitions may affect coordination
- Bandwidth costs for cross-region traffic

Best for: Organizations with offices in multiple locations, research collaborations across institutions.

### Hybrid Cloud/On-Premise

Combining cloud and local resources:

```
Cloud Provider              On-Premise
├── Gateway (public IP)     ├── Gateway (public IP)
├── Workers (spot/preempt)  ├── Workers (stable)
└── Data Node               └── Data Node

    Scheduler (either location)
```

Benefits:
- Burst to cloud for additional capacity
- Keep sensitive data on-premise
- Cost optimization through spot instances

Challenges:
- Egress costs from cloud providers
- Latency between cloud and on-premise
- Security policies for hybrid connectivity

Best for: Enterprises with existing infrastructure supplementing with cloud capacity.

### Edge Computing

Training models across edge devices:

```
Central Site                Edge Locations (many)
├── Gateway                 ├── Worker (device 1)
├── Scheduler               ├── Worker (device 2)
└── Data Node               └── ...
```

Benefits:
- Privacy through local computation
- Reduced data transfer to central sites
- Personalization with local data

Challenges:
- Device heterogeneity
- Unreliable connectivity
- Limited compute per device

Best for: IoT deployments, mobile device fleets, privacy-sensitive applications.

## Communication Patterns

### Task Allocation Flow

1. Scheduler publishes `RequestWorker` message to `hypha/worker` topic via Gossipsub
2. Workers evaluate request against local resources and pricing threshold
3. Qualified workers send `WorkerOffer` messages directly to scheduler via request/response
4. Scheduler selects best offer(s) and sends `RenewLease` to accept
5. Worker confirms lease renewal and awaits job dispatch

### Training Flow

1. Worker requests data slice from scheduler
2. Scheduler assigns available slice and returns data node peer ID
3. Worker opens stream to data node and fetches slice
4. Worker processes batches and reports metrics to scheduler
5. Scheduler determines when to schedule DiLoCo update
6. Scheduler sends `ScheduleUpdate` message with counter to worker
7. After counter batches, worker sends pseudo-gradient to parameter server
8. Parameter server aggregates, applies Nesterov momentum, broadcasts weights
9. Worker receives updated weights and continues training

### Failure Handling

Failures are detected through lease mechanisms:

- Workers that stop renewing leases are considered failed
- Schedulers that miss renewal deadlines lose resource reservations
- Lease expiration triggers automatic cleanup without explicit failure messages

This approach provides fault detection without requiring perfect failure detectors or consensus protocols.

## Next Steps

Understanding Hypha's architecture provides the foundation for deploying and operating the system. Component-specific sections offer detailed configuration guidance:

- [Scheduler Configuration](scheduler.md)
- [Gateway Setup](gateway.md)
- [Worker Configuration](worker.md)
- [Data Node Deployment](data-node.md)
- [Security Setup](security.md)
