<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-scheduler CLI Reference

This document contains the help content for the `hypha-scheduler` command-line program.

**Command Overview:**

* [`hypha-scheduler`↴](#hypha-scheduler)
* [`hypha-scheduler init`↴](#hypha-scheduler-init)
* [`hypha-scheduler probe`↴](#hypha-scheduler-probe)
* [`hypha-scheduler run`↴](#hypha-scheduler-run)

## `hypha-scheduler`

The Hypha Scheduler discovers workers via the Hypha network and orchestrates
distributed ML training jobs.


**Usage:** `hypha-scheduler <COMMAND>`

###### **Subcommands:**

* `init` — Generate a default configuration file
* `probe` — Check if a remote peer is healthy and reachable
* `run` — Start the scheduler and begin job orchestration



## `hypha-scheduler init`

Generate a default configuration file

Creates a TOML configuration file with sensible defaults for job orchestration,
including certificate paths, network addresses, gateway connections, and job settings.

IMPORTANT: If the output file exists, it will be overwritten without warning.


**Usage:** `hypha-scheduler init [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path where the configuration file will be written

  Default value: `config.toml`
* `-n`, `--name <NAME>` — Name of this scheduler node
* `-j`, `--job <JOB>`



## `hypha-scheduler probe`

Check if a remote peer is healthy and reachable

Connects to the specified multiaddr, sends a health check request, and exits
with code 0 if the peer is healthy, or non-zero otherwise.

Useful for:
* Verifying gateway connectivity before starting jobs
* Container health checks (Docker, Kubernetes)
* Monitoring scheduler availability
* Deployment verification and readiness checks

NOTE: It's not possible to self-probe using the same certificate used to run the scheduler.


**Usage:** `hypha-scheduler probe [OPTIONS] <ADDRESS>`

###### **Arguments:**

* `<ADDRESS>` — Target peer multiaddr to probe

   Examples:
     /ip4/192.168.1.100/tcp/8080/
     /dns4/scheduler.example.com/tcp/443/p2p/12D3KooW...

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



## `hypha-scheduler run`

Start the scheduler and begin job orchestration

Loads configuration, connects to gateways, discovers workers, and orchestrates
training jobs. Runs until interrupted (SIGINT/SIGTERM) with a graceful shutdown.


**Usage:** `hypha-scheduler run [OPTIONS]`

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
* `--gateway <GATEWAY_ADDRESSES>` — Gateway addresses to connect to (repeatable, overrides config)

   Gateways provide network bootstrapping, DHT access, and optional relay.

   Examples:
     --gateway /ip4/203.0.113.10/tcp/8080/
     --gateway /dns4/gateway.hypha.example/tcp/443/
* `--listen <LISTEN_ADDRESSES>` — Addresses to listen on (repeatable, overrides config)

   Where the scheduler accepts incoming connections.

   Examples:
     --listen /ip4/0.0.0.0/tcp/9090
     --listen /ip4/0.0.0.0/udp/9090/quic-v1
* `--external <EXTERNAL_ADDRESSES>` — External addresses to advertise (repeatable, overrides config)

   Publicly reachable addresses peers should use to connect.

   Examples:
     --external /ip4/203.0.113.20/tcp/9090
     --external /dns4/scheduler.example.com/tcp/9090
* `--relay-circuit <RELAY_CIRCUIT>` — Enable relay circuit listening via gateway (overrides config)

   true = use relay (default), false = direct connections only.

  Possible values: `true`, `false`

* `--exclude-cidr <EXCLUDE_CIDR>` — CIDR ranges to exclude from DHT (repeatable, overrides config)

   Filters out peer addresses matching these ranges before adding to the DHT.
   Examples: 10.0.0.0/8, fc00::/7
