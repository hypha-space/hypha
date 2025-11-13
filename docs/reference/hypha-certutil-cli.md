<!-- NOTE: Auto-generated. Do not edit manually. -->

# hypha-certutil CLI Reference

This document contains the help content for the `hypha-certutil` command-line program.

**Command Overview:**

* [`hypha-certutil`↴](#hypha-certutil)
* [`hypha-certutil root`↴](#hypha-certutil-root)
* [`hypha-certutil org`↴](#hypha-certutil-org)
* [`hypha-certutil node`↴](#hypha-certutil-node)

## `hypha-certutil`

Certificate generation and management tool for the Hypha network.

Creates a three-tier certificate hierarchy:

* Root CA - Central certificate authority (stored securely, used rarely)
* Organization CAs - Intermediate CAs for each tenant/organization
* Node Certificates - End-entity certificates for individual services

Uses Ed25519 exclusively for compatibility with libp2p and strong security with
small key sizes. All private keys are stored in PKCS#8 format as required by
the Hypha network.


**Usage:** `hypha-certutil <COMMAND>`

Important! For production use, it is not recommended to use this tool as it is not designed for high security and scalability. Instead, consider using a dedicated PKI management tool or a third-party service providers.

###### **Subcommands:**

* `root` — Generate a Root CA certificate (top of PKI hierarchy)
* `org` — Generate an Organization/Tenant CA (intermediate CA)
* `node` — Generate a node certificate (end-entity certificate)



## `hypha-certutil root`

Generate a Root CA certificate (top of PKI hierarchy)

Creates the root certificate authority that forms the trust anchor for your entire
PKI. This certificate signs Organization CAs, which in turn sign node certificates.


OUTPUT FILES:
* `<org>-root-ca-cert.pem` - Root CA certificate (public, distribute to all nodes)
* `<org>-root-ca-key.pem` - Root CA private key (KEEP SECURE, never share)

The certificate uses Ed25519 algorithm and includes basic constraints marking it
as a CA certificate with path length constraint.


**Usage:** `hypha-certutil root [OPTIONS] --organization <ORGANIZATION>`

Example:

```
hypha-certutil root -o 'ACME Corporation' --country US -d /secure/root-ca
```


###### **Options:**

* `-o`, `--organization <ORGANIZATION>` — Organization name (appears in certificate subject)

   This name identifies the entity that operates the PKI. Choose a name that
   clearly identifies your organization for certificate validation.

   Examples: "ACME Corporation", "Hypha Dev", "Example University"
* `--country <COUNTRY>` — Country code (ISO 3166-1 alpha-2, 2 letters)

   Two-letter country code for the certificate subject. Common values:
   * US - United States
   * GB - United Kingdom
   * DE - Germany
   * CN - China

   While optional, including country helps with certificate identification.

  Default value: `US`
* `-n`, `--name <NAME>` — Common name for the Root CA (defaults to "<org> Root CA")

   The CN field in the certificate subject. If not specified, automatically
   generated as "<organization> Root CA".

   Best practice: Include "Root CA" in the name for easy identification.

   Examples: "ACME Production Root CA", "Hypha Test Root CA 2025"
* `-d`, `--dir <DIR>` — Output directory for certificate and private key files

   Directory where the Root CA certificate and private key will be saved.
   The directory will be created if it doesn't exist.

   SECURITY: Use a secure location with restricted access:
   * Encrypted filesystem
   * Access controls (chmod 600 recommended)

  Default value: `.`



## `hypha-certutil org`

Generate an Organization/Tenant CA (intermediate CA)

Creates an intermediate CA certificate signed by the Root CA. Organization CAs
represent tenants or organizational units in the Hypha network. Each tenant gets
their own CA that can issue node certificates, providing cryptographic isolation.

OUTPUT FILES:
* `<org>-ca-cert.pem` - Organization CA certificate (public)
* `<org>-ca-key.pem` - Organization CA private key (KEEP SECURE)
* `<org>-ca-trust.pem` - Trust chain (Org CA + Root CA, for node configs)

The trust chain file bundles the Organization CA and Root CA certificates,
making it easy to configure nodes with the complete trust chain.


**Usage:** `hypha-certutil org [OPTIONS] --root-cert <ROOT_CERT> --root-key <ROOT_KEY> --organization <ORGANIZATION>`

Example: Create Organization CA for tenant 'acme-corp'

```
hypha-certutil org --root-cert certs/root/root-ca-cert.pem
  --root-key certs/root/root-ca-key.pem
  -o acme-corp -d certs/tenants/acme
```


###### **Options:**

* `--root-cert <ROOT_CERT>` — Path to the Root CA certificate file

   The Root CA certificate that will sign this Organization CA. Must be
   a valid PEM-encoded X.509 certificate.

   Typically a `*-cert.pem` generated via 'hypha-certutil root'
* `--root-key <ROOT_KEY>` — Path to the Root CA private key file

   The Root CA's private key used to sign this Organization CA certificate.
   Must be in PKCS#8 PEM format.

   SECURITY: This operation requires access to the Root CA private key.

   Typically a `*-key.pem` generated via 'hypha-certutil root'
* `-o`, `--organization <ORGANIZATION>` — Organization/tenant name (e.g., acme-corp, globex)

   Identifies the tenant or organizational unit. This name appears in the
   certificate subject and is used in output filenames.

   Choose a name that:
   * Clearly identifies the tenant
   * Is filesystem-safe (no special characters)
   * Is consistent with your naming conventions

   Examples: "acme-corp", "engineering-dept", "tenant-001"
* `-n`, `--name <NAME>` — Common name for the Organization CA (defaults to "<org> CA")

   The CN field in the certificate subject. If not specified, automatically
   generated as "<organization> CA".

   Examples: "ACME Corp Intermediate CA", "Engineering Department CA"
* `-d`, `--dir <DIR>` — Output directory for certificate and key files

   Directory where the Organization CA certificate, private key, and trust
   chain will be saved. Created if it doesn't exist.

  Default value: `.`



## `hypha-certutil node`

Generate a node certificate (end-entity certificate)

Creates a certificate for an individual node, service, or component in the Hypha
network. Node certificates are signed by an Organization.

These certificates are used by:

* Gateways - Network entry points
* Schedulers - Job orchestrators
* Workers - Task executors
* Data nodes - Dataset providers

OUTPUT FILES:
* `<name>-cert.pem` - Node certificate (public)
* `<name>-key.pem` - Node private key (KEEP SECURE, chmod 600)
* `<name>-trust.pem` - Complete trust chain (Org CA + Root CA)

The trust chain enables the node to validate peer certificates by including
the full CA hierarchy.


**Usage:** `hypha-certutil node [OPTIONS] --ca-cert <CA_CERT> --ca-key <CA_KEY> --name <NAME>`

Example: simple scheduler certificate

```
hypha-certutil node
    -n scheduler
    --ca-cert acme-ca-cert.pem
    --ca-key acme-ca-key.pem
    -d certs/nodes/scheduler-01
```


###### **Options:**

* `--ca-cert <CA_CERT>` — Path to the Organization CA certificate file

   The CA certificate that will sign this node certificate. Typically an Organization CA, but could be the Root CA for testing.

   Typically: `<org>-ca-cert.pem` from 'hypha-certutil org'
* `--ca-key <CA_KEY>` — Path to the Organization CA private key file

   The CA's private key used to sign this node certificate. Must be in
   PKCS#8 PEM format.

   SECURITY: Protect this key as it can issue certificates for the organization.

   Typically: `<org>-ca-key.pem` from 'hypha-certutil org'
* `-n`, `--name <NAME>` — Common name for the node certificate

   Unique identifier for this node. Best practices:
   * Use FQDN format: <hostname>.<tenant>.<domain>
   * Include node type: gateway-01, scheduler-prod, worker-gpu-03
   * Keep consistent naming convention across deployment
   * Avoid special characters (use hyphens, not underscores)

   The CN is automatically added to SANs if not explicitly included.
* `-s`, `--san <SAN>` — Subject Alternative Names (SANs) - DNS names and IP addresses

   Comma-separated list of DNS names and/or IP addresses that this certificate
   will be valid for. Critical for TLS validation and peer connectivity.

   The common name (-n) is automatically included if not in this list.

   NOTE: 0.0.0.0 is included by default for local testing. Override with
   specific addresses for production.

  Default value: `0.0.0.0`
* `-d`, `--dir <DIR>` — Output directory for certificate and key files

   Directory where the node certificate, private key, and trust chain will
   be saved. Created if it doesn't exist.

   SECURITY: Set restrictive permissions on this directory (chmod 700)
   and especially on `*-key.pem` files (chmod 600).

  Default value: `.`
