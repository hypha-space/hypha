<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-data CLI Reference

This document contains the help content for the `hypha-data` command-line program.

**Command Overview:**

* [`hypha-data`↴](#hypha-data)
* [`hypha-data init`↴](#hypha-data-init)
* [`hypha-data probe`↴](#hypha-data-probe)
* [`hypha-data run`↴](#hypha-data-run)

## `hypha-data`

The Hypha Data serves datasets to workers via the Hypha network.

Data peers connect to gateways, announce their datasets via the DHT, and respond
to data fetch requests from workers during training. Each dataset is divided into
slices (files) that can be distributed across multiple workers for parallel processing.


**Usage:** `hypha-data <COMMAND>`

###### **Subcommands:**

* `init` — Generate a default configuration file
* `probe` — Check if a remote peer is healthy and reachable
* `run` — Start the data node and begin serving datasets



## `hypha-data init`

Generate a default configuration file

Creates a TOML configuration file with sensible defaults for dataset serving.
The generated config includes certificate paths, network addresses, gateway
connections, and dataset path configuration.

IMPORTANT: If the output file exists, it will be overwritten without warning.


**Usage:** `hypha-data init [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path where the configuration file will be written

  Default value: `config.toml`
* `-n`, `--name <NAME>` — Name of this data node
* `-d`, `--dataset-path <DATASET_PATH>` — Path or file providing a dataset



## `hypha-data probe`

Check if a remote peer is healthy and reachable

Connects to the specified multiaddr, sends a health check request, and exits
with code 0 if the peer responds as healthy, or non-zero otherwise.

Useful for:
* Verifying gateway connectivity before announcing datasets
* Container health checks (Docker, Kubernetes)
* Monitoring data node availability
* Deployment verification and readiness checks

NOTE: It's not possible to self-probe using the same certificate used to run the data node.


**Usage:** `hypha-data probe [OPTIONS] <ADDRESS>`

###### **Arguments:**

* `<ADDRESS>` — Target peer multiaddr to probe

   Examples:
     /ip4/192.168.1.100/tcp/8080/
     /dns4/data.example.com/tcp/443/p2p/12D3KooW...

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



## `hypha-data run`

Start the data node and begin serving datasets

Loads configuration, connects to gateways, and enters the main serving loop.
The process runs until interrupted (SIGINT/SIGTERM), then performs graceful
shutdown to ensure data transfers are properly terminated.


**Usage:** `hypha-data run [OPTIONS]`

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
