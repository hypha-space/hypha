# Request-Response Example

This example demonstrates how to build a simple request-response application using the Hypha network, featuring mutual TLS (mTLS) authentication. This example provides a hands-on guide for securely authenticating a client and server using certificates and a simple request/response Protocol. You’ll see how the server efficiently manages multiple concurrent requests, illustrating essential Hypha network operations.

## Prerequisites

Let’s prepare the ground! Before running the example, you need to generate the required certificates using the `hypha-certutil`.

> [!CRITICAL]
> The `hypha-certutil` is perfect for testing and development purposes. However, for production setups, ensure you’re using a proper, PKI setup.

First, generate a Root CA:

```sh
cargo run --bin hypha-certutil -- root \
    --organization "Hypha Example" \
    --name "example-root-ca"
```

Next, generate the Server Certificate (signed by the root CA):

```sh
cargo run --bin hypha-certutil -- node \
    --ca-cert example-root-ca-cert.pem \
    --ca-key example-root-ca-key.pem \
    --name "server.example.local"
```
Then, generate the Client Certificate (also signed by the root CA):

```sh
cargo run --bin hypha-certutil -- node \
    --ca-cert example-root-ca-cert.pem \
    --ca-key example-root-ca-key.pem \
    --name "client.example.local"
```

By signing these certificates with the Root CA, you’re establishing a trust between the client and server. They both trust certificates signed by this CA.

> [!TIP]
> It’s also possible to introduce additional layers of CAs (like organizational CAs) for multi-tenant setups, establishing a more sophisticated chain of trust.

After running these commands, you’ll have:
- `server-example-local-cert.pem`
- `server-example-local-key.pem`
- `server-example-local-trust.pem`
- `client-example-local-cert.pem`
- `client-example-local-key.pem`
- `client-example-local-trust.pem`

Now that we’ve generated all the PEM files, you’re ready to roll!

## Running the Example

> [!TIP]
> Don't forget to enable logging with `RUST_LOG=info` to see the logs from the client and server.

Launch the server in your terminal:

```sh
RUST_LOG=info cargo run --example request_response -- \
    --cert-file server-example-local-cert.pem \
    --key-file server-example-local-key.pem \
    --trust-file server-example-local-trust.pem \
    server \
    --listen-addr "/ip4/127.0.0.1/tcp/8080"
```

In another terminal, connect the client:

```sh
RUST_LOG=info cargo run --example request_response -- \
    --cert-file client-example-local-cert.pem \
    --key-file client-example-local-key.pem \
    --trust-file client-example-local-trust.pem \
    client \
    --server-addr "/ip4/127.0.0.1/tcp/8080"
```

You should see informative logs demonstrating secure, authenticated communication between your server and client.
