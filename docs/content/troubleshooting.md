+++
title = "Troubleshooting"
description = "Aid in resolving common issues encountered when using Hypha."
weight = 30
[taxonomies]
track = ["reference"]
+++

# Troubleshooting

Something not working? This guide helps you diagnose issues, verify your setup, and find fixes for problems others have encountered. Work through the tools below to gather evidence, then match your symptoms to known failure signatures—or [open an issue](https://github.com/hypha-space/hypha/issues/new?template=bug.md) if you hit something new.

> [!TIP]
> If you're still provisioning a new environment, complete the [Quick Start](@/quickstart.md) first so the baseline configuration is correct before diving into debugging.

## Troubleshooting Tools

Before chasing errors, use these tools to understand what's happening and verify that nodes can actually reach each other.

### Control log verbosity with `RUST_LOG`

Set the `RUST_LOG` environment variable when you start a Hypha binary to raise or reduce noise per module.

```bash
# hypha-worker will emit debug logs while QUIC transport stays at info
RUST_LOG=hypha_worker=debug,libp2p_quic=info hypha-worker --config worker.toml
```
```log
2025-11-27T12:15:46.111Z DEBUG hypha_worker::executor::bridge requesting data slice index from scheduler
2025-11-27T12:15:46.219Z  INFO hypha_worker::executor::bridge copied resource size=268435456 file=/tmp/hypha/artifacts/0
```

Adjust the comma-separated filter for each crate (`hypha_gateway`, `hypha_scheduler`, `hypha_data`, etc.) to isolate the subsystem that is misbehaving. Use `trace` only temporarily; it may capture sensitive payload details and bloats log volume.

### Capture telemetry with OpenTelemetry (OTEL)

Centralizing logs and traces removes guesswork when multiple peers interact. Run a collector (e.g., Grafana Cloud OTLP endpoint, or a local collector) and point each Hypha binary at it via the standard `OTEL_*` variables or the respective configuration options.

Start a local collector (replace image/tag as needed):
```bash
docker run --rm -p 4317:4317 otel/opentelemetry-collector-contrib:0.140.0
```

Now, configure Hypha to export spans and metrics to the collector and then start Hypha as usual:
```bash
export OTEL_EXPORTER_OTLP_ENDPOINT="http://127.0.0.1:4317"
export OTEL_EXPORTER_OTLP_PROTOCOL="grpc"
export OTEL_SERVICE_NAME="worker-lab-01"
export OTEL_RESOURCE_ATTRIBUTES="service.namespace=lab,deployment.environment=staging"

hypha-worker --config worker.toml
```

> [!WARNING]
> Telemetry payloads contain peer IDs and certificate fingerprints. Only forward OTLP data to collectors that enforce encryption.

For hosted collectors (Grafana Cloud, AWS OTEL, etc.), reuse the same environment variables but update `OTEL_EXPORTER_OTLP_ENDPOINT`, `OTEL_EXPORTER_OTLP_HEADERS`, and `OTEL_EXPORTER_OTLP_PROTOCOL` according to the provider. Keep the service name unique per node so traces from the gateway, scheduler, workers, and data nodes remain distinguishable.

### Inspect the public IP that peers advertise

Mismatched public IPs lead to relay loops and errors. Confirm what the Internet sees before editing `exclude_cidr` or firewall rules.

```bash
curl https://1.1.1.1/cdn-cgi/trace/ | grep ip=
```
```text
ip=203.0.113.24
```

### Get a peer ID from a certificate

The `hypha-inspect cert-info` command derives the libp2p PeerID from a certificate or private key file. Use it when you have a node's certificate but need its PeerID to look up its addresses or check logs.

```bash
hypha-inspect cert-info /path/to/node-cert.pem
```
```text
PeerId: 12D3KooWExamplePeerId
```

A typical troubleshooting workflow: get the PeerID from a certificate, look up its advertised addresses in the DHT, then probe those addresses to verify connectivity.

### Look up a peer in the DHT

The `hypha-inspect lookup` command queries gateways to find a peer's advertised addresses in the DHT. This helps diagnose discovery issues—if a peer isn't in the DHT, other nodes can't find it.

```bash
hypha-inspect lookup --config worker.toml 12D3KooWExamplePeerId
```
```text
PeerId: 12D3KooWExamplePeerId
Addresses:
  /ip4/203.0.113.10/udp/9000/quic-v1
  /ip4/10.0.0.5/udp/9000/quic-v1
```

No results? The peer may not have successfully connected to a gateway, or its DHT records expired. Restart the peer and check its logs for gateway connection errors.

### Probe a peer directly

The `probe` command checks whether a remote peer is healthy and reachable. Pass any multiaddr from the lookup output—the peer ID suffix is optional.

> [!NOTE]
> All Hypha binaries (except `hypha-certutil`) include the `probe` subcommand. You can run `hypha-worker probe`, `hypha-gateway probe`, etc., whichever is already configured on the machine you're troubleshooting from.

```bash
hypha-inspect probe --config worker.toml /ip4/203.0.113.10/udp/9000/quic-v1
```
```text
Peer 12D3KooWExamplePeerId is healthy (response time: 23ms)
```

If the probe fails, check firewall rules, confirm both nodes use certificates from the same trust chain, and verify the target peer is running.

See also: [hypha-inspect CLI reference](@/reference/hypha-inspect-cli.md) for all available options.

## Failure Signatures

This section documents issues we've seen in the field—bugs that have since been fixed, common misconfigurations, and edge cases. Some entries simply require upgrading to a newer release; others need configuration changes on your end.

If your error isn't listed here, please [open an issue](https://github.com/hypha-space/hypha/issues/new?template=bug.md) with debug logs attached. Either we'll help you fix a misconfiguration, or you've found something new that we should address and document.

### Snappy decompression failure

```text
cramjam.DecompressionError: snappy: corrupt input (expected valid offset but got offset 882; dst position: 0)
```

```log
2025-11-26T09:38:35.115470Z  INFO hypha_worker::executor::bridge: Copied resource size=131 file=/tmp/hypha-df71010f-4977-41d2-a694-b4ffde3a590b/artifacts/0
Traceback (most recent call last):
  File "/Users/test/.cache/uv/archive-v0/wu732xbcarPOjBnemLBfq/lib/python3.12/site-packages/snappy/snappy.py", line 84, in uncompress
    out = bytes(_uncompress(data))
                ^^^^^^^^^^^^^^^^^
cramjam.DecompressionError: snappy: corrupt input (expected valid offset but got offset 882; dst position: 0)
```

#### What this indicates

- The worker fails immediately after starting a training task.
- The logged `size=<value>` next to `hypha_worker::executor::bridge` shows the bytes Hypha copied from the dataset. A very small size (for example, `size=131`) usually means Git LFS pointers were synced instead of the real files.
- If the copied size matches the real dataset file size, the artifacts are likely compressed with the wrong codec.

#### How to fix it

1. **Confirm the dataset fully downloaded.** Run `cat <file>` in the dataset repository. If the output looks like a Git LFS pointer (starts with `version https://git-lfs.github.com/spec/v1`), delete the repo, install/configure `git lfs`, and clone again so the binary artifacts download instead of pointers.
2. **Verify compression format.** When the logged `size` matches the on-disk dataset file size, ensure every dataset file is compressed with [Snappy](https://github.com/google/snappy). Recompress offending files and re-upload them before retrying the training run.

See also: [Training guide](@/training.md) for dataset packaging and [Worker reference](@/worker.md) for artifact mounting paths.

### GLIBC version mismatch

```text
hypha-gateway: /lib/x86_64-linux-gnu/libc.so.6: version `GLIBC_2.38' not found (required by hypha-gateway)
```
```text
hypha-gateway: /lib64/libc.so.6: version `GLIBC_2.38' not found (required by hypha-gateway)
```

#### What this indicates

- You are running a Hypha build that dynamically links against glibc 2.38, but the host OS ships an older glibc.
- These binaries were produced before we switched to statically linked MUSL builds, so they only work on recent distributions.

#### How to fix it

1. **Upgrade Hypha to alpha.17 or newer.** Starting with `v1.0.0-alpha.17`, all Linux releases are built with MUSL and statically linked.

See also: [Installation](@/installation.md) for supported platforms and package formats.

### Relay circuit exhaustion

**Worker Logs (captured with RUST_LOG=debug)**
```log
2025-11-27 11:34:25.325 error bridge error: network
2025-11-27 11:34:25.325 error Outbound request-response failure
2025-11-27 11:34:25.324 debug Closed connection
2025-11-27 11:34:25.324 debug Peer disconnected
2025-11-27 11:34:25.324 debug Connection closed with error IO(Custom { kind: Other, error: Custom { kind: Other, error: Error(Right(Decode(Io(Custom { kind: UnexpectedEof, error: "peer closed connection without sending TLS close_notify: https://docs.rs/rustls/latest/rustls/manual/_03_howto/index.html#unexpected-eof" })))) } }): Connected { endpoint: Listener { local_addr: /ip4/92.63.156.142/udp/55001/quic-v1/p2p-circuit, send_back_addr: /p2p/12D3KooWJfDWPLaH5Nh2qkD4JJkTYmAyDeNtZVtuo7LkTXRJfPce }, peer_id: PeerId("12D3KooWJfDWPLaH5Nh2qkD4JJkTYmAyDeNtZVtuo7LkTXRJfPce") }
...
2025-11-27 11:34:24.787 debug ffc4ce71: new outbound (Stream ffc4ce71/420) of (Connection ffc4ce71 Server (streams 4))
2025-11-27 11:34:24.786 debug Requesting data slice index from scheduler
``` 

**Gateway Logs (captured with RUST_LOG=debug)**
```log
2025-11-27 11:34:25.393 debug Unhandled event: Behaviour(BehaviourEvent: CircuitClosed { src_peer_id: PeerId("12D3KooWJfDWPLaH5Nh2qkD4JJkTYmAyDeNtZVtuo7LkTXRJfPce"), dst_peer_id: PeerId("12D3KooWRrp64o43d3CQovjUT52ojtNUTtkUpJ4wSd9CfyZjkRqp"), error: Some(Custom { kind: Other, error: "Max circuit bytes reached." }) })
2025-11-27 11:34:24.408 debug Unhandled event: Behaviour(BehaviourEvent: Event { peer: PeerId("12D3KooWJfDWPLaH5Nh2qkD4JJkTYmAyDeNtZVtuo7LkTXRJfPce"), connection: ConnectionId(7), result: Ok(23.343316ms) })
```

#### What this indicates

- The worker asks the scheduler for the next data slice, opens a stream through the gateway, and the connection closes before the transfer completes.
- Debug logs (enable with `RUST_LOG=debug` or use OpenTelemetry) show the gateway closing a `p2p-circuit` relay because the `max circuit bytes` limit was reached.
- This happens when peers cannot establish a direct path and large amounts of data are forced over the gateway. Common causes:
  - Multiple nodes on the same host or LAN without allowing the respective ranges in `exclude_cidr`
  - Firewall rules blocking direct peer connections
  - NAT traversal (DCUtR) failing silently

#### How to fix it

1. **Allow direct LAN/localhost connectivity.** Update each node's `exclude_cidr` so the relevant private ranges or `127.0.0.1` addresses are advertised, and ensure every peer listens on a unique port. See the [Multiple Nodes on Localhost or Private Network](@/deploy/local_multi_node.md) guide for detailed steps.
2. **Verify firewall/NAT rules.** Ensure workers, data nodes, and schedulers can reach each other directly or via DCUtR. Relays should only serve for DCUtR and as DHT anchors, not the primary channel for dataset transfers.
3. **Monitor for residual relays.** With direct routes in place, you should no longer see `Max circuit bytes reached` events. If they persist, double-check that all peers restarted with the new configuration and that no infrastructure component forces traffic back through the gateway.

See also: [Gateway reference](@/gateway.md) for relay settings and [Worker reference](@/worker.md) for `exclude_cidr` options.
