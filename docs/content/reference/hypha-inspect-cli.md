+++
title = "hypha-inspect CLI"
description = "Auto-generated reference for the hypha-inspect command, covering configuration and operational subcommands."
[taxonomies]
track = ["reference"]
+++

<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-inspect CLI Reference

This document contains the help content for the `hypha-inspect` command-line program.

**Command Overview:**

* [`hypha-inspect`↴](#hypha-inspect)
* [`hypha-inspect probe`↴](#hypha-inspect-probe)
* [`hypha-inspect lookup`↴](#hypha-inspect-lookup)
* [`hypha-inspect cert-info`↴](#hypha-inspect-cert-info)
* [`hypha-inspect init`↴](#hypha-inspect-init)

## `hypha-inspect`

Hypha Inspect is a CLI tool for diagnosing network connectivity,
inspecting routing tables, and verifying identities within the Hypha network.


**Usage:** `hypha-inspect <COMMAND>`

###### **Subcommands:**

* `probe` — Check if a remote peer is healthy and reachable
* `lookup` — Lookup a peer in the DHT via gateways
* `cert-info` — Derive PeerID from a certificate file
* `init` — Generate a default configuration file



## `hypha-inspect probe`

Connects to the specified multiaddr and performs a health check.

In verbose mode, prints detailed routing information and connection stats.


**Usage:** `hypha-inspect probe [OPTIONS] <ADDRESS>`

###### **Arguments:**

* `<ADDRESS>` — Target peer multiaddr to probe

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
* `--timeout <TIMEOUT>` — Maximum time to wait for health response (milliseconds)

   If the peer doesn't respond within this duration, the probe fails.
   Increase for high-latency networks or overloaded peers.

  Default value: `2000`
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



## `hypha-inspect lookup`

Connects to configured gateways and performs a DHT lookup for the given PeerID.
Prints the routing table information (closest peers, addresses) found.


**Usage:** `hypha-inspect lookup [OPTIONS] <PEER_ID>`

###### **Arguments:**

* `<PEER_ID>` — The PeerID to lookup

###### **Options:**

* `-c`, `--config <CONFIG_FILE>` — Path to the configuration file

  Default value: `config.toml`
* `--gateway <GATEWAY_ADDRESSES>` — Gateway addresses to connect to (repeatable, overrides config)

   Gateways provide network bootstrapping, DHT access, and optional relay.

   Examples:
     --gateway /ip4/203.0.113.10/tcp/8080/
     --gateway /dns4/gateway.hypha.example/tcp/443/
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



## `hypha-inspect cert-info`

Reads an X.509 certificate or private key PEM file and prints the corresponding
libp2p PeerID.


**Usage:** `hypha-inspect cert-info <PATH>`

###### **Arguments:**

* `<PATH>` — Path to the PEM file (certificate or private key)



## `hypha-inspect init`

Generate a default configuration file

Creates a TOML configuration file with sensible defaults.


**Usage:** `hypha-inspect init [OPTIONS]`

###### **Options:**

* `-o`, `--output <OUTPUT>` — Path where the configuration file will be written

  Default value: `config.toml`
* `--gateway <GATEWAY_ADDRESSES>` — Gateway addresses to connect to (repeatable, overrides config)

   Gateways provide network bootstrapping, DHT access, and optional relay.

   Examples:
     --gateway /ip4/203.0.113.10/tcp/8080/
     --gateway /dns4/gateway.hypha.example/tcp/443/
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
