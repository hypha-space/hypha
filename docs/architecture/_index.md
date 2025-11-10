+++
title = "Architecture"
description = "Overview of Hypha's architecture and design principles"
weight = 2
sort_by = "weight"
+++

# Architecture Overview

Hypha is built on a decentralized, peer-to-peer architecture designed to democratize access to large-scale machine learning infrastructure while maintaining enterprise-grade security and reliability.

## Design Principles

Hypha's architecture is guided by six core principles, prioritized in this order:

1. **Secure**: Security, data sovereignty, and privacy are foundational. The system emphasizes secure defaults, confidential computing, comprehensive authentication, and strict access controls.

2. **Real-world Ready**: Design decisions account for real-world constraints including network limitations, regulatory compliance, and diverse infrastructure challenges.

3. **Intuitive**: Implementations are coherent, easy to understand, and straightforward, balancing simplicity with necessary complexity.

4. **Operationally Autonomous**: The system operates independently with minimal user intervention through automated resource discovery, workload distribution, fault tolerance, and scaling.

5. **Compatible**: Supports diverse, heterogeneous hardware setups commonly found in enterprises and research institutions.

6. **Performant**: Ensures efficient and optimized capabilities without compromising security.

## System Components

Hypha consists of four main components:

### [Network Layer](/architecture/network/)

The foundational P2P networking layer built on libp2p, providing:
- Peer discovery via Kademlia DHT
- Secure transport with mTLS
- NAT traversal and relay capabilities
- Request/response and pub/sub messaging

### [Gateway Nodes](/architecture/gateway/)

Stable network entry points that:
- Act as bootstrap peers for network discovery
- Provide relay services for NAT traversal
- Maintain stable addresses for peer connectivity
- Enable WebRTC and other browser-compatible transports

### [Scheduler Nodes](/architecture/scheduler/)

Coordinate task distribution by:
- Discovering available worker nodes
- Matching tasks to worker capabilities
- Distributing ML workloads efficiently
- Monitoring task execution and health

### [Worker Nodes](/architecture/worker/)

Execute ML tasks by:
- Advertising computational capabilities
- Receiving and executing task assignments
- Reporting task progress and results
- Managing local resources efficiently

## Communication Patterns

### Peer Discovery

Nodes discover each other through:
1. **Bootstrap peers**: Initial connection to known gateway nodes
2. **Kademlia DHT**: Distributed hash table for peer routing
3. **mDNS**: Local network discovery (optional)
4. **Identify protocol**: Exchange of peer capabilities and addresses

### Task Distribution

Tasks flow through the system as:
1. **Task Submission**: Client submits task to scheduler
2. **Worker Discovery**: Scheduler queries DHT for capable workers
3. **Task Assignment**: Scheduler dispatches task to selected worker
4. **Execution**: Worker executes task and reports progress
5. **Result Delivery**: Worker returns results to requester

### Fault Tolerance

The system handles failures through:
- **Heartbeat monitoring**: Detect unresponsive nodes
- **Task reassignment**: Redistribute failed tasks
- **Peer redundancy**: Multiple paths for peer connectivity
- **State replication**: Critical state stored across multiple nodes

## Security Model

### Transport Security

- **mTLS**: Mutual TLS authentication for all peer connections
- **Certificate-based identity**: Cryptographic peer identities
- **Secure channels**: Encrypted communication by default

### Access Control

- **Capability-based**: Workers advertise specific capabilities
- **Task authorization**: Verify task submitter permissions
- **Resource isolation**: Sandboxed task execution environments

### Data Privacy

- **Local computation**: Data processed on worker nodes
- **Encrypted transport**: All data encrypted in transit
- **Data sovereignty**: Control over where data is processed

## Scalability

Hypha scales through:

- **Horizontal scaling**: Add more worker and gateway nodes
- **Decentralized coordination**: No single point of failure
- **Efficient routing**: DHT-based peer discovery
- **Load distribution**: Scheduler balances across workers

## Next Steps

- [Network Architecture](/architecture/network/) - Deep dive into the P2P layer
- [Gateway Nodes](/architecture/gateway/) - Understanding gateway operations
- [Scheduler Nodes](/architecture/scheduler/) - How task scheduling works
- [Worker Nodes](/architecture/worker/) - Worker node internals
