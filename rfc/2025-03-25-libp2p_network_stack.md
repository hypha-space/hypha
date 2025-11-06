# libp2p based network stack

## Overview

This RFC proposes adopting libp2p as the foundation for our decentralized network stack, targeting scalability, efficiency, and modularity. It seeks to deliver high-throughput data transfers, robust DHT-based peer discovery, hole punching, and flexible network behaviors. Adopting libp2p's well-supported ecosystem reduces operational overhead and accelerates development.

## Background

Hypha requires a scalable network stack capable of handling substantial data volumes while ensuring robust and efficient peer communication. Our initial choice of libp2p, particularly its Rust implementation,  presented throughput challenges (limited to around 50–60 MB/s even on local loopback). These limitations prompted exploration of alternative frameworks and custom implementations despite our initial choice. Some convictions need to be tested and validated.

Alternatives such as custom gRPC-based stacks, Litep2p, Iroh, and HiveMind were considered (again) but ultimately dismissed due to complexity, limited community support, or performance trade-offs.

## Proposal

We propose adopting libp2p as the core network framework, addressing initial performance concerns through targeted optimizations:

- **High-Throughput Streaming:** Leverage TCP with the latest yamux multiplexer implementation for dynamic window sizing. Use the libp2p stream API to facilitate asynchronous parallel data transfers via task spawning, thereby improving throughput from the initial 60 MB/s to approximately 1 GB/s without any additional optimizations.

- **Modular Interface Design:** Provide a simple abstraction/pattern to simplify the integration of new network behaviors. This will minimize boilerplate code and focus developer efforts on core logic.

- **Fallback Mechanisms:** Should the native libp2p throughput prove insufficient, implement a fallback strategy using Tokio-based custom high-throughput streaming over TCP or UDP. This approach will ensure that we can bypass any limitations of the P2P network stack when necessary.

- **Ecosystem Benefits:** Leverage libp2p’s extensive community and contributor base (250+ active contributors, utilized by over 22,000 projects) to ensure ongoing development, reliability, and support.

## Experimentation

- Throughput Improvement: Early experiments significantly improved libp2p Rust implementation throughput by:
  - Adopting the latest yamux multiplexer supporting dynamic window sizing.
  - Utilizing asynchronous parallel streams via Tokio’s task spawning, reaching approximately 1 GB/s throughput without extensive optimization.

- Simple Abstractions: A preliminary implementation pattern demonstrates handling network actions through clearly defined traits and asynchronous patterns.

## Abandoned Ideas

During our evaluation, several alternative approaches were considered but ultimately set aside:

* Custom gRPC-based (or simple http based) Stack: While potentially capable of achieving higher raw message throughput, a custom solution would forgo the benefits of the established libp2p ecosystem and introduce additional complexity. [exo](https://github.com/exo-explore/exo) is an example for a _p2p_ machine learning framework which rolled its own networking stack based on gRPC.

* Alternative Libraries/Frameworks: Projects such as [Litep2P](https://github.com/paritytech/litep2p) and [Iroh](https://github.com/n0-computer/iroh/tree/main/iroh), though interesting, either lacked the community support or the comprehensive feature set of libp2p.

* HiveMind as the foundation: [HiveMind](https://github.com/learning-at-home/hivemind) while mostly implemented in Python uses the libp2p-go imaplementation as network driver managed via Python. While it comes with many of the features we look to provide the added layers not only come with performance toll (data conversion to from the libp2p sidecar daemon) they also make the deployment/operations harder.
