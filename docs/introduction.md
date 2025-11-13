# Introduction

## What is Hypha?

Hypha is a self-managing "Kubernetes for AI" designed for distributed machine learning training and inference. At its core, Hypha enables organizations to train and serve massive models across heterogeneous, poorly-connected infrastructure—from HPC GPU farms to commodity hardware—without requiring centralized coordination.

Built on libp2p for decentralized networking, Hypha maintains the security and reliability enterprises require while eliminating single points of failure. The system implements DiLoCo (Distributed Low-Communication) style training, an approach that dramatically reduces communication overhead compared to traditional data-parallel training.

### Key Features

**Self-Managing Infrastructure:** Hypha automatically handles resource discovery, workload distribution, fault tolerance, and scaling with minimal configuration. Deploy nodes with their capabilities and let the system handle coordination—no manual orchestration required.

**Decentralized Networking:** Built on libp2p's proven peer-to-peer protocols, Hypha creates a resilient network with no single point of failure. Nodes discover each other automatically and communicate directly, eliminating the brittleness of centralized architectures.

**Efficient Communication:** DiLoCo-based training reduces network communication by approximately 500x compared to traditional data-parallel approaches. Workers perform extensive local training before synchronizing, making large-scale training practical even over limited bandwidth or high-latency connections.

**Heterogeneous Hardware Support:** Pool diverse hardware—from high-end datacenter GPUs to modest commodity devices—into a unified training infrastructure. Performance-aware scheduling automatically allocates work according to each node's capabilities, maximizing utilization across your entire fleet.

**Secure Permissioned Network:** Mutual TLS authentication and certificate-based access control secure every connection. All network communication is encrypted end-to-end, and certificate revocation provides immediate, network-wide access control when needed.

## Core Goals and Use Cases

### Democratize Large-Scale ML

Large-scale machine learning has traditionally required substantial capital investment in homogeneous GPU clusters—infrastructure that remains out of reach for most research institutions and enterprises. Hypha makes this capability accessible by enabling organizations to pool existing heterogeneous resources that might otherwise sit idle: compute capacity across departments, labs, offices, or even partner organizations.

Research institutions can combine resources from multiple labs to tackle models they couldn't train individually. Enterprises can utilize spare capacity across regional datacenters during off-peak hours. Organizations train models that would otherwise require renting expensive cloud infrastructure—or simply wouldn't be trained at all.

### Minimize Operational Complexity

Traditional distributed training systems require careful orchestration of resource allocation, network configuration, and failure handling. Hypha's self-managing architecture automates these concerns.

The system discovers available resources through the DHT, negotiates resource allocation through a decentralized protocol, and handles failures through automatic lease expiration. Operators configure nodes with their capabilities and let the system handle coordination.

### Support Production-Grade Inference

Beyond training, Hypha provides a reliable inference backbone for production applications. The same decentralized architecture that enables distributed training also supports scalable, resilient inference serving.

Request routing, load balancing, and failure handling happen automatically through the peer-to-peer network. Services can scale horizontally by adding workers, and the network adapts without reconfiguration.

### Enable Training on Distributed Infrastructure

Many organizations have compute resources spread across multiple locations with varying network connectivity. Traditional training systems assume fast, reliable connections between all nodes.

Hypha's DiLoCo implementation specifically targets this scenario. By reducing synchronization frequency and tolerating heterogeneous performance, the system can train effectively even when workers have limited bandwidth or unreliable connections.

Edge computing scenarios, multi-region deployments, and hybrid cloud/on-premise setups all benefit from this capability.

## Getting Started

For hands-on experience with Hypha, proceed to the [Quick Start](quick-start.md) guide to set up your first end-to-end decentralized training system.
