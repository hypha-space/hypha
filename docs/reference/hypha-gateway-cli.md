<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-gateway CLI Reference

This document contains the help content for the `hypha-gateway` command-line program.

**Command Overview:**

* [`hypha-gateway`↴](#hypha-gateway)
* [`hypha-gateway init`↴](#hypha-gateway-init)
* [`hypha-gateway probe`↴](#hypha-gateway-probe)
* [`hypha-gateway run`↴](#hypha-gateway-run)

## `hypha-gateway`

The Hypha Gateway is a stable, publicly accessible entry point for the Hypha P2P
network. It provides bootstrapping, relaying, and DHT participation for discovery.


**Usage:** `hypha-gateway <COMMAND>`

###### **Subcommands:**

* `init` — Generate a default configuration file
* `probe` — Check if a remote peer is healthy and reachable
* `run` — Start the gateway and begin serving the network



## `hypha-gateway init`

Generate a default configuration file

Creates a TOML configuration file with sensible defaults for running a gateway,
including certificate paths and network addresses.

IMPORTANT: If the output file exists, it will be overwritten without warning.


**Usage:** `hypha-gateway init [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path where the configuration file will be written

  Default value: `config.toml`
* `-n`, `--name <NAME>` — Name of this gateway node



## `hypha-gateway probe`

Check if a remote peer is healthy and reachable

Connects to the specified multiaddr, sends a health check request, and exits
with code 0 if the peer is healthy, or non-zero otherwise.

Useful for:
* Container health checks (Docker, Kubernetes)
* Monitoring and alerting
* Deployment verification and readiness checks

NOTE: It's not possible to self-probe using the same certificate used to run the gateway.


**Usage:** `hypha-gateway probe [OPTIONS] <ADDRESS>`

###### **Arguments:**

* `<ADDRESS>` — Target peer multiaddr to probe

   Examples:
     /ip4/192.168.1.100/tcp/8080/
     /dns4/gateway.example.com/tcp/443/p2p/12D3KooW...

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

   Used to load TLS certificates for secure connection to the target peer.

  Default value: `config.toml`
* `--cert <CERT_PEM>` — Path to the certificate PEM file (overrides config)

   Must be a valid X.509 certificate in PEM format. If not provided, uses
   cert_pem from the configuration file.
* `--key <KEY_PEM>` — Path to the private key PEM file (overrides config)

   Must correspond to the certificate. If not provided, uses key_pem from
   the configuration file.

   SECURITY: Ensure this file has restricted permissions (e.g., chmod 600).
* `--trust <TRUST_PEM>` — Path to the trust chain PEM file (overrides config)

   CA bundle containing certificates trusted by this node. If not provided,
   uses trust_pem from the configuration file.
* `--crls <CRLS_PEM>` — Path to the certificate revocation list PEM (overrides config)

   Optional CRL for rejecting compromised certificates. If not provided,
   uses crls_pem from the configuration file if present.
* `--timeout <TIMEOUT>` — Maximum time to wait for health response (milliseconds)

   If the peer doesn't respond within this duration, the probe fails.
   Increase for high-latency networks or overloaded peers.

  Default value: `2000`



## `hypha-gateway run`

Start the gateway and begin serving the network

Loads configuration, starts listeners, participates in the DHT and relay,
and runs until interrupted (SIGINT/SIGTERM) with a graceful shutdown.


**Usage:** `hypha-gateway run [OPTIONS]`

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
* `--cert <CERT_PEM>` — Path to the certificate PEM file (overrides config)

   Must be a valid X.509 certificate in PEM format.
* `--key <KEY_PEM>` — Path to the private key PEM file (overrides config)

   Must correspond to the certificate. Security: restrict permissions (e.g., chmod 600).
* `--trust <TRUST_PEM>` — Path to the trust chain PEM file (overrides config)

   CA bundle containing certificates trusted by this node. If not provided,
   uses trust_pem from the configuration file.
* `--crls <CRLS_PEM>` — Path to the certificate revocation list PEM (overrides config)

   Optional CRL for rejecting compromised certificates. If not provided,
   uses crls_pem from the configuration file if present.
* `--listen <LISTEN_ADDRESSES>` — Addresses to listen on (repeatable, overrides config)

   Where this gateway accepts incoming connections.
   Examples: /ip4/0.0.0.0/tcp/8080, /ip4/0.0.0.0/udp/8080/quic-v1
* `--external <EXTERNAL_ADDRESSES>` — External addresses to advertise (repeatable, overrides config)

   Publicly reachable addresses peers should use to connect.
   Examples: /ip4/203.0.113.10/tcp/8080, /dns4/gateway.example.com/tcp/8080
* `--exclude-cidr <EXCLUDE_CIDR>` — CIDR ranges to exclude from DHT (repeatable, overrides config)

   Filters out peer addresses matching these ranges before adding to the DHT.
   Examples: 10.0.0.0/8, fc00::/7
