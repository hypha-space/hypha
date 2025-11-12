# Security

Hypha implements enterprise-grade security through mutual TLS (mTLS) authentication, Certificate Authority-based access control, and comprehensive auditing. This section covers security architecture, certificate management, and operational security considerations.

## mTLS Setup

Mutual TLS provides bidirectional authentication where both client and server verify each other's identities using certificates signed by a trusted Certificate Authority.

### Certificate Authority Infrastructure

Hypha requires a PKI hierarchy for certificate issuance:

**Root CA**: Ultimate trust anchor. Private key must be kept offline and highly secured.

**Intermediate CA** (optional): Issued by root CA, used for day-to-day certificate signing. Compromise of intermediate CA doesn't require root key rotation.

**Node Certificates**: Individual certificates for each Hypha node (gateway, scheduler, worker, data node).

**Hierarchy Example**:
```
Root CA
 └── Intermediate CA (Organization)
      ├── Gateway-01 Certificate
      ├── Gateway-02 Certificate
      ├── Scheduler-01 Certificate
      ├── Worker-01 Certificate
      ├── Worker-02 Certificate
      └── Data-01 Certificate
```

### Certificate Requirements

**Key Algorithm**: Ed25519 only (initially)

Hypha currently supports only Ed25519 keys. Future versions may add RSA and ECDSA (secp256r1 curve) support, but secp256k1 will not be supported for Web PKI compatibility.

**Key Format**: PKCS#8 for private keys

Private keys must be in PKCS#8 format:
```
-----BEGIN PRIVATE KEY-----
...
-----END PRIVATE KEY-----
```

**Certificate Format**: X.509 PEM-encoded

Certificates must be PEM-encoded:
```
-----BEGIN CERTIFICATE-----
...
-----END CERTIFICATE-----
```

**CA-Signed Only**: No self-signed certificates

All node certificates must be signed by a trusted CA. Self-signed certificates are not supported.

**Certificate Validity**: Reasonable periods (typically 1 year)

Short validity periods (e.g., 90 days to 1 year) improve security through regular rotation.

**No libp2p Extensions**: Standard X.509 certificates

Unlike standard libp2p which uses custom X.509 extensions for peer identity, Hypha uses standard certificates without extensions.

### Certificate Distribution

**Deploying Certificates to Nodes**:

1. Generate certificate signing request (CSR) on each node or centrally
2. Sign CSR with CA to produce certificate
3. Distribute certificate and private key to node via secure channel
4. Install CA certificate bundle (trust chain) on all nodes

**File Locations** (configurable):
```
/etc/hypha/certs/
├── ca-bundle.pem           # CA certificates (root + intermediates)
├── gateway-01-cert.pem     # Node certificate
├── gateway-01-key.pem      # Node private key (0600 permissions)
└── crls.pem                # Certificate Revocation List (optional)
```

**Secure Storage**:
- Private keys: 0600 permissions (readable only by owner)
- Certificates: 0644 permissions (publicly readable)
- CA bundle: 0644 permissions

**File Permissions Example**:
```bash
chmod 600 /etc/hypha/certs/*-key.pem
chmod 644 /etc/hypha/certs/*-cert.pem
chmod 644 /etc/hypha/certs/ca-bundle.pem
```

### Certificate Generation

For development and testing, use `hypha-certutil`:

**Generate Root CA**:
```bash
hypha-certutil gen-ca \
    --out-cert root-ca-cert.pem \
    --out-key root-ca-key.pem \
    --common-name "Hypha Root CA"
```

**Generate Intermediate CA** (optional):
```bash
hypha-certutil gen-intermediate-ca \
    --ca-cert root-ca-cert.pem \
    --ca-key root-ca-key.pem \
    --out-cert intermediate-ca-cert.pem \
    --out-key intermediate-ca-key.pem \
    --common-name "Hypha Intermediate CA"
```

**Generate Node Certificate**:
```bash
hypha-certutil gen-cert \
    --ca-cert intermediate-ca-cert.pem \
    --ca-key intermediate-ca-key.pem \
    --out-cert gateway-01-cert.pem \
    --out-key gateway-01-key.pem \
    --common-name "gateway-01.hypha.local"
```

**Create CA Bundle**:
```bash
cat intermediate-ca-cert.pem root-ca-cert.pem > ca-bundle.pem
```

**Production Note**: For production deployments, use established PKI infrastructure (HashiCorp Vault, AWS Certificate Manager, etc.) rather than `hypha-certutil`.

### Certificate Revocation Lists (CRLs)

CRLs enable immediate access revocation without waiting for certificate expiration.

**Generating CRL**:

Use your CA tooling to generate a CRL listing revoked certificates. Example with OpenSSL:
```bash
openssl ca -gencrl -out crls.pem -config ca.conf
```

**Distributing CRL**:

Copy CRL to all nodes:
```bash
scp crls.pem node:/etc/hypha/certs/crls.pem
```

**Configuring Nodes**:
```toml
crls_pem = "/etc/hypha/certs/crls.pem"
```

**Updating CRL**:

Current implementation loads CRLs at startup only. To update:
1. Generate new CRL with revoked certificates
2. Distribute to all nodes
3. Restart nodes to load new CRL

**Future Enhancement**: Dynamic CRL refresh without restart (planned).

## Node Authentication Flow

Hypha uses mutual TLS for bidirectional authentication:

### mTLS Handshake

**1. Connection Initiation**: Client node connects to server node.

**2. Server Certificate Presentation**: Server sends its certificate to client.

**3. Client Verification**: Client validates server certificate:
   - Certificate signature valid (signed by trusted CA)
   - Certificate not expired
   - Certificate not in CRL
   - Certificate chain verified to trusted root

**4. Client Certificate Presentation**: Client sends its certificate to server.

**5. Server Verification**: Server validates client certificate using same checks.

**6. Connection Established**: Both nodes authenticated, encrypted channel established.

### Certificate Chain Validation

Hypha uses rustls `WebPkiClientVerifier` for certificate validation:

**Validation Steps**:
1. Verify certificate signature using CA public key
2. Check certificate validity period (not before / not after)
3. Verify certificate chain to trusted root CA
4. Check certificate revocation status (if CRL provided)
5. Ensure certificate is properly formatted

**Trust Anchor**: The CA bundle (`trust_pem`) contains root and intermediate CA certificates. Any certificate signed by a CA in this bundle is trusted.

### PeerID Derivation

Unlike standard libp2p which maintains separate stable identity keys, Hypha derives PeerIDs directly from certificate public keys.

**Derivation**:
```
PeerID = multihash(certificate_public_key)
```

**Implication**: When certificates rotate, PeerIDs change. This is a trade-off accepted for PKI compatibility and simplified key management.

**Impact**:
- Peer discovery mechanisms must handle PeerID changes
- Connection configurations may need updates after certificate rotation
- Monitoring and logging should use stable identifiers (hostnames) in addition to PeerIDs

### No SNI (Server Name Indication)

Following libp2p conventions, Hypha disables SNI during TLS handshake.

**Rationale**:
- Maintains compatibility with libp2p extensibility requirements
- Simplifies initial implementation
- Avoids complexity of dynamic certificate selection

**Future Consideration**: SNI support may be added for scenarios requiring dynamic certificate signing or multi-tenant deployments.

## Permissioned Decentralized Network Overview

Hypha creates a permissioned P2P network where trust is rooted in organizational CAs rather than open, permissionless participation.

### Organizational Trust Boundaries

**Certificate-Based Membership**: Only nodes with CA-signed certificates can join the network.

**CA as Access Control**: The CA determines network membership. Revoking a certificate immediately removes access.

**Multi-Organization Support**: Different organizations can participate by sharing root CAs or using cross-signed intermediates.

### Separation from Standard libp2p Networks

**Incompatibility with Self-Signed Certificates**: Hypha nodes cannot communicate with standard libp2p nodes using self-signed certificates.

**Separate Network**: Hypha forms a distinct network isolated from open libp2p networks.

**Design Choice**: This isolation ensures enterprise security requirements (CA-signed certificates, revocation) are enforced uniformly.

### PeerID Stability Trade-Off

**Standard libp2p**: Stable PeerIDs persist across certificate rotations via separate identity keys.

**Hypha**: PeerIDs change on certificate rotation because they derive from certificate keys.

**Justification**:
- Simplified key management (one keypair instead of two)
- Certificate as single source of truth for identity
- Alignment with standard PKI practices
- Acceptable for Hypha's use case (planned rotations, centralized coordination)

## Transport Security

All Hypha network communication is encrypted and authenticated.

### TLS 1.3

**Protocol**: TLS 1.3 for all connections

**Cipher Suites**: Configured by rustls (modern, secure defaults)

**Perfect Forward Secrecy**: TLS 1.3 provides PFS automatically

### TCP Transport

**Current Support**: TCP with TLS 1.3 (mTLS)

**Configuration**:
```toml
listen_addresses = [
    "/ip4/0.0.0.0/tcp/4001"
]
```

**Encryption**: All TCP connections use TLS 1.3 with mTLS authentication.

### QUIC Transport

**Status**: QUIC transport available in libp2p but not compatible with Hypha's mTLS implementation.

**Limitation**: QUIC in libp2p uses standard libp2p certificates (self-signed with custom extensions), not CA-signed certificates.

**Future Work**: QUIC support with custom certificate validation may be added in future releases.

### No Unencrypted Transports

Hypha does not support unencrypted transports. All network communication is encrypted via TLS 1.3.

## Job Bridge Security

The job bridge provides controlled network access to executor processes.

### Unix Socket Permissions

**Socket Creation**: Each job receives a unique Unix domain socket.

**Permissions**: Sockets created with 0600 permissions (owner read/write only).

**Isolation**: Socket only accessible to spawning user (same user running worker).

### Process Isolation

**Separate Processes**: Each job executes as a separate process.

**Work Directory Isolation**: Each job receives isolated work directory with 0700 permissions.

**Resource Limits**: Future work to implement cgroups or similar for CPU/memory limits.

### Path Validation

The job bridge enforces strict path security:

**No Absolute Paths**: Requests with absolute paths are rejected.

**No Parent Traversal**: Paths containing `..` are rejected.

**Work Directory Confinement**: All paths must be relative to job's work directory.

**Example Validation**:
```rust
// Allowed
"models/resnet.safetensors"  // relative, within work dir

// Blocked
"/etc/passwd"                 // absolute path
"../other-job/data"           // parent traversal
"/tmp/../etc/passwd"          // absolute + traversal
```

### Protection Against Malicious Schedulers

**Assumption**: Schedulers and peers may be compromised or malicious.

**Defenses**:
- Path validation prevents directory escape
- No shell command execution from scheduler-provided data
- Executor processes run with worker's privileges (not elevated)

**Limitation**: Malicious schedulers can send invalid or malformed data, but cannot escape work directory or access other jobs' data.

### Job Isolation Limitations

**Current Implementation**: No isolation between jobs on the same worker.

**Security Risk**: A malicious job could potentially:
- Access other jobs' work directories
- Receive inputs meant for another job
- Interfere with other jobs' execution

**Planned Enhancement**: Job-scoped resource access to prevent cross-job interference.

**Workaround**: Run workers in separate VMs or containers to provide kernel-level isolation.

## Future Security Enhancements

Several security improvements are planned:

### Dynamic CRL Refresh

**Current**: CRLs loaded at node startup only.

**Enhancement**: Periodic CRL refresh without restart.

**Benefit**: Faster revocation without operational disruption.

### Hardware Security Module (HSM) Integration

**Goal**: Store private keys in HSMs rather than filesystem.

**Benefit**: Protect keys from compromise even if server is compromised.

**Use Cases**: High-security deployments, compliance requirements.

### Automatic Certificate Rotation

**Goal**: Rotate certificates automatically before expiration.

**Approach**: Integration with CA APIs for automated CSR submission and certificate retrieval.

**Benefit**: Reduced operational burden, improved security posture.

### Multi-CA Federation

**Goal**: Support multiple trusted CAs for federated deployments.

**Use Case**: Multi-organization collaborations where each org maintains its own CA.

**Approach**: CA bundle includes multiple root CAs, nodes trust certificates from any.

### Job-Scoped Resource Access

**Goal**: Prevent jobs from accessing each other's data or network resources.

**Approach**:
- Scope job bridge API calls to specific job IDs
- Validate peer connections match expected job participants
- Isolate data slice assignments per job

**Benefit**: Defense against malicious jobs and stronger multi-tenancy.

## Security Best Practices

### Certificate Management

**Protect Private Keys**:
- Store with restrictive permissions (0600)
- Never transmit over unencrypted channels
- Consider HSM for high-value keys

**Regular Rotation**:
- Rotate certificates before expiration
- Shorter validity periods (90 days - 1 year) improve security
- Plan rotation windows to minimize disruption

**CA Key Protection**:
- Keep root CA key offline
- Use intermediate CAs for daily operations
- Implement multi-party authorization for CA operations

**CRL Maintenance**:
- Update CRLs promptly when revoking certificates
- Automate CRL distribution to all nodes
- Monitor CRL expiration and renewal

### Network Security

**Firewall Configuration**:
- Restrict inbound connections to necessary ports
- Use security groups or iptables for defense in depth
- Monitor for unauthorized connection attempts

**Gateway Hardening**:
- Gateways have elevated attack surface (public IPs)
- Implement rate limiting for connection attempts
- Monitor for unusual traffic patterns

**Monitoring**:
- Log all authentication failures
- Alert on certificate validation errors
- Track unusual connection patterns

### Operational Security

**Principle of Least Privilege**:
- Run worker processes as non-root users
- Limit filesystem permissions to necessary directories
- Separate worker user accounts per environment

**Audit Logging**:
- Enable comprehensive logging
- Centralize logs for analysis
- Retain logs for forensic investigation

**Incident Response**:
- Plan for certificate compromise (rotation procedures)
- Test revocation workflows
- Document escalation procedures

**Multi-Tenancy Considerations**:
- Current job isolation limitations mean workers should not be shared between untrusted tenants
- Use separate worker pools per tenant for strong isolation
- Consider VM or container isolation for defense in depth

## Troubleshooting Security Issues

**Certificate Validation Failures**:
- Check certificate not expired: `openssl x509 -in cert.pem -noout -dates`
- Verify certificate chain: `openssl verify -CAfile ca-bundle.pem cert.pem`
- Ensure trust bundle includes all intermediate CAs
- Check file permissions on certificates and keys

**Connection Refused**:
- Verify firewall allows connections on configured ports
- Check listen addresses are correctly configured
- Ensure certificates are valid for both client and server

**PeerID Mismatches After Rotation**:
- Update configurations with new PeerIDs
- Use DNS names or stable identifiers in configurations
- Monitor logs for connection attempts with old PeerIDs

**CRL Issues**:
- Verify CRL file format: `openssl crl -in crls.pem -noout -text`
- Check CRL is not expired
- Ensure CRL includes expected revoked certificates
- Restart nodes after CRL updates (until dynamic refresh implemented)

**Key Format Errors**:
- Ensure keys are in PKCS#8 format, not traditional format
- Convert if necessary: `openssl pkcs8 -topk8 -in key.pem -out key.pkcs8.pem`
- Verify Ed25519 algorithm (not RSA, ECDSA, etc.)

## Next Steps

- Deploy [Gateway](gateway.md) nodes with mTLS configured
- Configure [Workers](worker.md), [Schedulers](scheduler.md), and [Data Nodes](data-node.md) with certificates
- Review [Operations](operations.md) for security monitoring
- Establish certificate rotation procedures
- Test revocation workflows
