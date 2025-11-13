<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-worker CLI Reference

This document contains the help content for the `hypha-worker` command-line program.

**Command Overview:**

* [`hypha-worker`↴](#hypha-worker)
* [`hypha-worker init`↴](#hypha-worker-init)
* [`hypha-worker probe`↴](#hypha-worker-probe)
* [`hypha-worker run`↴](#hypha-worker-run)

## `hypha-worker`

The Hypha Worker executes training and inference jobs assigned by schedulers via
the Hypha network.


**Usage:** `hypha-worker <COMMAND>`

###### **Subcommands:**

* `init` — Generate a default configuration file
* `probe` — Check if a remote peer is healthy and reachable
* `run` — Start the worker and begin accepting jobs



## `hypha-worker init`

Generate a default configuration file

Creates a TOML configuration file with sensible defaults for job execution,
including certificate paths, network addresses, gateway connections, and
resource/executor settings.

IMPORTANT: If the output file exists, it will be overwritten without warning.


**Usage:** `hypha-worker init [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path where the configuration file will be written

  Default value: `config.toml`
* `-n`, `--name <NAME>` — Name of this worker node



## `hypha-worker probe`

Check if a remote peer is healthy and reachable

Connects to the specified multiaddr, sends a health check request, and exits
with code 0 if the peer is healthy, or non-zero otherwise.

Useful for:
* Verifying gateway connectivity before starting jobs
* Container health checks (Docker, Kubernetes)
* Monitoring worker availability
* Deployment verification and readiness checks

NOTE: It's not possible to self-probe using the same certificate used to run the worker.


**Usage:** `hypha-worker probe [OPTIONS] <ADDRESS>`

###### **Arguments:**

* `<ADDRESS>` — Target peer multiaddr to probe

   Examples:
     /ip4/192.168.1.100/tcp/8080/
     /dns4/worker.example.com/tcp/443/p2p/12D3KooW...

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



## `hypha-worker run`

Start the worker and begin accepting jobs

Loads configuration, connects to gateways, advertises resources, and executes
assigned jobs. Runs until interrupted (SIGINT/SIGTERM) with a graceful shutdown.


**Usage:** `hypha-worker run [OPTIONS]`

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
* `--gateway <GATEWAY_ADDRESSES>` — Gateway addresses to connect to (repeatable, overrides config)

   Gateways provide network bootstrapping, DHT access, and optional relay.
   Must include the peer ID in the multiaddr.

   Examples:
     --gateway /ip4/203.0.113.10/tcp/8080/p2p/12D3KooWAbc...
     --gateway /dns4/gateway.hypha.example/tcp/443/p2p/12D3KooWAbc...
   Required: connect to at least one gateway.
* `--listen <LISTEN_ADDRESSES>` — Addresses to listen on (repeatable, overrides config)

   Where the worker accepts incoming connections.

   Examples:
     --listen /ip4/0.0.0.0/tcp/9091
     --listen /ip4/0.0.0.0/udp/9091/quic-v1
* `--external <EXTERNAL_ADDRESSES>` — External addresses to advertise (repeatable, overrides config)

   Publicly reachable addresses peers should use to connect.

   Examples:
     --external /ip4/203.0.113.30/tcp/9091
     --external /dns4/worker.example.com/tcp/9091
* `--relay-circuit <RELAY_CIRCUIT>` — Enable relay circuit listening via gateway (overrides config)

   true = use relay (default), false = direct connections only.

  Possible values: `true`, `false`

* `--socket <SOCKET_ADDRESS>` — Socket path for driver communication (overrides config)

   Unix domain socket for worker-executor communication (optional).
* `--work-dir <WORK_DIR>` — Base directory for job working directories (overrides config)

   Where per-job working directories are created.

   Examples:
     --work-dir /tmp
     --work-dir /mnt/fast-ssd/hypha
* `--exclude-cidr <EXCLUDE_CIDR>` — CIDR ranges to exclude from DHT (repeatable, overrides config)

   Filters out peer addresses matching these ranges before adding to the DHT.

   Examples: 10.0.0.0/8, fc00::/7
