# Contributing

This guide will help you understand the overall organization of the project. It's the single source of truth for how to contribute to the code base.

> [!NOTE]
> The key words "MUST", "MUST NOT", "REQUIRED", "SHALL", "SHALL NOT", "SHOULD","SHOULD NOT", "RECOMMENDED", "MAY", and "OPTIONAL" in this document are to be interpreted as described in [RFC 2119](https://tools.ietf.org/html/rfc2119).

## Principles

These principles guide every aspect of design and implementation, underpinning the core goal to make large-scale machine learning accessible by removing barriers and enabling widespread innovation. Each principle represents a core quality describing how the system SHOULD be:

1. **Secure**

Security, data sovereignty, and privacy are foundational and non-negotiable. Emphasize secure defaults, confidential computing, comprehensive authentication, and strict access controls to responsibly handle data.

2. **Operationally Autonomous**

The system should operate independently and reliably with minimal user intervention. Address resource constraints and operational complexity by automating resource discovery, workload distribution, fault tolerance, and scaling, significantly lowering operational barriers.

3. **Real-world Ready**

Real-world constraints like network limitations, regulatory compliance, and diverse infrastructure challenges should inform design decisions. Proactively accommodate these realities, ensuring seamless integration into existing environments.

4. **Intuitive**

Ensure designs and implementations are coherent, easy to understand, and straightforward, whether at the system level or within individual features and components. Balance "How simple can we make it?" with "How complex does it have to be?"  ([Laws of Simplicity / Reduce](http://lawsofsimplicity.com/los/law-1-reduce.html)) and answer "What goes with what?" ([Laws of Simplicity / Organize](http://lawsofsimplicity.com/los/law-2-organize.html)).
Prioritize clear structure and streamlined interactions to reduce cognitive load and foster trust.

5. **Performant**

Ensure efficient and optimized training and inference capabilities. Performance is essential for practical usability, enabling demanding real-world applications to meet high standards for scale, latency, and reliability.

6. **Compatible**

Seamlessly integrate diverse, heterogeneous hardware setups commonly found in enterprises and research institutions. Supporting varied infrastructure unlocks the latent potential of underutilized hardware, enabling innovative ideas previously hindered by infrastructure limitations.

### Examples

The following are illustrative scenarios showing how these principles can be applied, especially when principles might conflict:

- *Operationally Autonomous and Intuitive*: Integrate and implement services, when possible instead of relying on separate services which would come with extra operetational cost and complexities.
- *Secure vs. Performant*: Even if bypassing encryption significantly enhances performance, security must never be compromised. Always choose secure defaults, such as encryption and authentication, despite potential performance trade-offs.
- *Real-world Ready vs. Performant*: While certain network protocols like QUIC might offer better performance, ensure there's a fallback mechanism (e.g., WebRTC or WebSockets) to handle realistic network constraints, such as environments blocking UDP traffic.
- *Intuitive vs. Performant*: Avoid overly complex configurations—even if they deliver maximum performance—if they compromise usability. Prioritize straightforward, intuitive setups to ensure users don't require extensive system knowledge for effective operation.

## Setup

Install the stable toolchain for building and the nightly toolchain for
formatting:

```sh
rustup toolchain install stable
rustup toolchain install nightly --component rustfmt
```

Format the codebase using `cargo +nightly fmt` before committing.

## Components

> [!IMPORTANT]
> We will probably split this mono-repo into multiple ones to separate the different components and their responsibilities better, allow for easier maintenance and development, as well as ease license handling.

The project is organized into multiple components (crates), each with its own purpose and responsibilities.

```mermaid
flowchart RL
    classDef transparent opacity:0

    TODO["TODO: Add component overview"]
```

<!-- TODO: Add short overview describing the main components purpose. -->

### Adding Components

New components SHOULD be added using `cargo new crates/${NAME}`. This will create a new crate directory with the standard Rust project structure.

After creating a new crate, you MUST link the appropriate license:

```sh
ln -s LICENSE-${TYPE} crates/${NAME}/LICENSE-${TYPE}
```

Where `${TYPE}` refers to the license type (e.g., `APACHE` for Apache-2.0).

Make sure to also update the license metadata in each crate's `Cargo.toml` file to correctly reflect the license being used:

```toml
[package]
# Other package metadata...
license = "${TYPE}"
```

The default license is Apache-2.0, but the project uses multiple licenses for the time being. If you're uncertain about which license to use, please consult the project lead. Make sure to use the SPDF license identifier, see  https://spdx.org/licenses/ for more information.

### Adding Dependencies

When adding dependencies, these MUST be added to the respective crate's `Cargo.toml` file. You can add dependencies using:

```sh
cargo add ${DEPENDENCY_NAME}
```

Make sure that the dependency is compatible with the project's licenses.

## Upgrade Dependencies

All dependencies SHALL be updated regularly to maintain an up to date and secure product. Updates SHOULD consider backward compatibility and MUST document compatibility issues.

<!-- TODO: Setup dependabot for automatic dependency updates and security updates and document configuration. -->

## Version Control

Changes SHOULD be committed frequently in small logical chunks that MUST be consistent, work independently of any later commits, and pass the linter plus the tests. Doing so eases rollback and rebase operations. Commits MUST not include any customer data.

Commit messages SHALL follow the [Conventional Commits specification](https://www.conventionalcommits.org/en/v1.0.0/). This provides a framework for explicit, readable messages and enables automated changelog generation.

## AI Assistants

> [!TIP]
> For _AI Agent_ guidance on effective collaboration on this project, please refer to the `AGENTS.md` file in the repository root.

The use of AI in open-source development is controversial and projects take varied approaches:

* **Full bans** – Projects like [Servo](https://book.servo.org/contributing.html#ai-contributions) reject any AI-generated code due to maintainability risks, quality concerns, and the challenges of reviewing code that contributors may not fully understand.
* **Embracing with guardrails** – Projects like Cloudflare's [`workers-oauth-provider`](https://github.com/cloudflare/workers-oauth-provider), written largely with Claude, demonstrate that rigorous review processes and clear guidelines can harness AI's productivity benefits while maintaining quality.

The Cloudflare example along with others show that with the right procedures, AI assistance can deliver on productivity promises while balancing risks. Our approach follows this second path, drawing inspiration from ["Field Notes From Shipping Real Code With Claude"](https://diwank.space/field-notes-from-shipping-real-code-with-claude) which observes:

> Good development practices aren't just nice-to-haves—they're the difference between AI that amplifies your capabilities versus your chaos.

Building on this insight, our `AGENTS.md` file and the principles below serve as a starting point for responsible AI collaboration practices. We believe that AI assistants can be powerful tools for increasing productivity when used with appropriate safeguards.

Using AI assistants (e.g., Gemini, Claude, ChatGPT) for development is encouraged to increase productivity. However, the human contributor is always ultimately responsible for the code they commit. Every use of AI assistants MUST follow the principles outlined below.

### Principles

* Accountability: The contributor, is accountable for any code committed. This responsibility is not diminished if the code was generated by an AI. Contributers MUST thoroughly review, understand, and test all AI-generated code to ensure it is correct, secure, and aligns with the project's principles and coding standards before submitting it.
* Attribution: If an AI assistant provides a significant contribution to a commit, it MUST be acknowledged using a `Co-Authored-By:` trailer in the commit message. This maintains transparency in the project's history.

### Attribution

Add the appropriate trailer on a new line in the commit message body:

* Claude: `Co-Authored-By: Claude <noreply@anthropic.com>`
* Gemini: `Co-Authored-By: Gemini <google@users.noreply.github.com>`
* ChatGPT: `Co-Authored-By: ChatGPT <openai@users.noreply.github.com>`

> [!NOTE]
> Not all AI assistant have an official Gihub account. Please use the organization name with the default github noreply email to prevent mentioning actual users when using a new assistant.
