# Introduction

## What is Hypha?

Hypha is a self-managing, enterprise-ready machine learning system designed for distributed training and inference. At its core, Hypha enables organizations to efficiently utilize heterogeneous compute resources across geographically distributed infrastructure.

Built on libp2p for decentralized networking, Hypha removes the need for centralized coordination while maintaining the security and reliability enterprises require. The system implements DiLoCo (Distributed Low-Communication), a federated learning approach that dramatically reduces communication overhead compared to traditional data-parallel training.

### Key Features

**Self-Managing Infrastructure**
Hypha automatically handles resource discovery, workload distribution, fault tolerance, and scaling with minimal configuration. The system continuously adapts to changing conditions without manual intervention.

**Decentralized Networking**
Using libp2p's proven peer-to-peer protocols, Hypha creates a resilient network where no single point of failure exists. Nodes communicate directly through Kademlia DHT for discovery, Gossipsub for pub-sub messaging, and high-throughput stream protocols for large data transfers.

**Efficient Communication**
The DiLoCo-based training algorithm reduces communication by approximately 500x compared to traditional data-parallel training. Workers perform many local training steps before synchronizing, making Hypha practical for networks with limited bandwidth or high latency.

**Heterogeneous Hardware Support**
Hypha works with diverse hardware configurations, from high-end datacenter GPUs to modest edge devices. Performance-aware scheduling ensures efficient utilization by allocating work according to each node's capabilities.

**Enterprise-Grade Security**
Mutual TLS (mTLS) authentication, Certificate Authority-based access control, and comprehensive auditing capabilities meet enterprise security requirements. All network communication is encrypted, and certificate revocation provides immediate access control.

## Core Goals and Use Cases

### Democratize Large-Scale ML

Large-scale machine learning has traditionally required substantial capital investment in homogeneous GPU clusters. Hypha makes this capability accessible to organizations by enabling them to pool existing heterogeneous resources across departments, offices, or partner organizations.

Research institutions can combine resources from multiple labs. Enterprises can utilize spare capacity across regional datacenters. Organizations can train models that would otherwise require renting expensive cloud infrastructure.

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

## Audience and Prerequisites

### Target Audience

This documentation serves:

- **ML Engineers** implementing and deploying machine learning models in production
- **DevOps Teams** managing distributed infrastructure and ensuring system reliability
- **Researchers** exploring distributed systems and federated learning approaches
- **System Architects** designing scalable ML platforms

### Prerequisites

Users should have:

**Machine Learning Fundamentals**
- Understanding of training loops, optimization, and model evaluation
- Familiarity with gradient descent and backpropagation
- Experience with ML frameworks like PyTorch or TensorFlow

**Programming Knowledge**
- Python proficiency for working with executors and data preparation
- Rust familiarity helpful but not required for basic usage
- Comfort with command-line tools and configuration files

**Systems Concepts**
- Basic networking knowledge (TCP, addressing, ports)
- Understanding of processes and resource management
- Experience with Linux/Unix systems preferred

Advanced topics like network protocol design, distributed systems theory, and cryptographic concepts are covered where relevant but not required for basic usage.

## Getting Started

For hands-on experience with Hypha, proceed to the [Installation](installation.md) guide to set up your first nodes. The [Architecture Overview](architecture.md) provides deeper understanding of how components interact, while component-specific sections offer detailed configuration and operational guidance.
