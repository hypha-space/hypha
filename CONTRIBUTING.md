# Contributing

This guide will help you understand the overall organization of the project. It's the single source of truth for how to contribute to the code base.

> [!TIP]
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

Commit message SHALL follow the guidelines stated here, as they provide a framework to write explicit messages that are easy to comprehend when looking through the project history and enable automatic changelog generation. Commit messages MUST never include references to individual customer.

The Guidelines are based on
[AngularJS Git Commit Message Conventions](https://goo.gl/27wkkO).

This project uses `commitlint` to ensure that messages follow the guidelines.
Run `yarn install` in the project root to install the respective git hooks.

### Commit-Message

Each commit message MUST consist of a header (type, subject), a body
and a footer separated by empty lines:

```
<type>: <subject>

<message>

<footer>
```

Any line of the commit message MUST NOT be longer than 100 characters to ensure
that the messages are easy to read.

#### Subject

The subject contains a succinct description of the change. It SHOULD use the
imperative and present tense; “change” not “changed” nor “changes”.
The first letter SHALL NOT be capitalized, and MUST NOT end it with a dot.

#### Type

The following commit types are allowed:

- **feat** -
  use this type for commits that introduce new features or capabilities
- **fix** - use this one for bug fixes
- **docs** - use this one to indicate documentation adjustments and improvements
- **refactor** -
  use this type for adjustments to improve maintainability or performance
- **test** - use this one for commits that add missing tests
- **chore** - use this type for _maintainance_ commits e.g. removing old files
- **ci** - use this type for CI adjustments
- **style** - use this one for commits that fix formatting and linting errors

#### Message

The message SHOULD describe the motivation for the change and contrast it with previous behavior. It SHOULD use the imperative and present tense.

#### Referencing Issues

Closed issues MUST be listed on a separate line in the footer prefixed with
"Closes" keyword.

#### Breaking Changes

All breaking changes MUST be mentioned in the footer with the description of
the change, justification, and migration notes. Start the block explaining the
breaking changes with the words `BREAKING CHANGE:` followed by a space.

### Examples

```
fix: remove UI log statements

Remove console log statements to prevent IE4 errors.

Closes #123, #456
```

```
fix: gracefully handle HTTP connections

Gracefully handle 4xx and 5xx status codes to allow for retries when applicable.

Closes #123
```

```
feat: add new Graphana data sources

Introduce a new Graphana data source

Closes #123
```

```
refactor: change constant names

Adjust constant names, following the new naming conventions.

Closes #123
```

```
refactor: simplify video control interface

Simplify the video player control interface as the current
interface is somewhat hard to use and caused bugs due
to accidental misuse.

BREAKING CHANGE: VideoPlayer control interface has changed
to simplify the general usage.

To migrate the code follow the example below:

Before:

VideoPlayer.prototype.stop({pause:true})

After:

VideoPlayer.prototype.pause()
```
