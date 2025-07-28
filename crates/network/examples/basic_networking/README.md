# Basic Networking Example

This example demonstrates the fundamental networking capabilities of the hypha-network crate, focusing on:

- **Error handling** with `HyphaError` types
- **Connection establishment** using dial and listen interfaces  
- **Basic network setup** without complex protocols
- **Driver pattern** for managing network events

## Features Demonstrated

### Error Handling
- `HyphaError::DialError` - Connection failures
- `HyphaError::SwarmError` - Swarm initialization issues
- Error trait implementations (Display, Debug, Error)

### Networking Traits
- `DialInterface` - Initiating connections to remote peers
- `ListenInterface` - Accepting incoming connections
- `DialDriver` - Processing dial events and connection state
- `ListenDriver` - Processing listen events and address management
- `SwarmDriver` - Core event loop and swarm management

### Basic Usage Patterns
- Creating network interfaces and drivers
- Setting up asynchronous event processing
- Managing connection lifecycle
- Handling network events and errors

## Running the Example

### Demo Mode (Recommended)
```bash
cargo run --example basic_networking demo
```

This creates two in-memory networks, connects them, and demonstrates error handling.

### Server Mode
```bash
cargo run --example basic_networking server --listen-addr "/memory/test_server"
```

### Client Mode  
```bash
cargo run --example basic_networking client --server-addr "/memory/test_server"
```

## Key Concepts

### Driver Pattern
The example shows the driver pattern used throughout hypha-network:
- **Interface**: High-level API for applications (`DialInterface`, `ListenInterface`)
- **Driver**: Event processing and state management (`DialDriver`, `ListenDriver`)
- **Action**: Commands sent from interface to driver (`DialAction`, `ListenAction`)

### Error Handling
All networking operations return `Result` types with appropriate error information:
```rust
match network.dial(address).await {
    Ok(peer_id) => println!("Connected to {}", peer_id),
    Err(e) => eprintln!("Connection failed: {}", e),
}
```

### Async Event Processing
The driver runs an event loop processing both libp2p swarm events and application actions:
```rust
tokio::select! {
    event = self.swarm.select_next_some() => {
        // Handle libp2p events
    }
    Some(action) = self.action_rx.recv() => {
        // Handle application actions
    }
}
```

## Testing

The example includes unit tests demonstrating:
- Basic connection setup
- Error type behavior
- Memory transport usage for testing

Run tests with:
```bash
cargo test --example basic_networking
```

## See Also

- `request_response` example - Higher-level request-response protocol
- `crates/network/src/dial.rs` - Dial trait implementation
- `crates/network/src/listen.rs` - Listen trait implementation
- `crates/network/src/error.rs` - Error type definitions