+++
title = "Contributing"
description = "How to contribute to the Hypha project"
weight = 4
+++

# Contributing to Hypha

Thank you for your interest in contributing to Hypha! This guide will help you get started with contributing to the project.

## Getting Started

Before you begin contributing, please:

1. Read the [Code of Conduct](https://github.com/hypha-space/hypha/blob/main/CODE_OF_CONDUCT.md)
2. Review the [CONTRIBUTING.md](https://github.com/hypha-space/hypha/blob/main/CONTRIBUTING.md) file in the repository
3. Familiarize yourself with the [Architecture](/architecture/)

## Ways to Contribute

There are many ways to contribute to Hypha:

- **Code**: Bug fixes, new features, performance improvements
- **Documentation**: Improve docs, add examples, write tutorials
- **Testing**: Write tests, report bugs, test new features
- **Design**: UX improvements, API design, architecture proposals
- **Community**: Answer questions, help users, organize events

## Development Setup

### Prerequisites

- Rust 1.75 or later
- Git
- A GitHub account

### Fork and Clone

1. Fork the repository on GitHub
2. Clone your fork locally:

```bash
git clone https://github.com/YOUR_USERNAME/hypha.git
cd hypha
```

3. Add the upstream repository:

```bash
git remote add upstream https://github.com/hypha-space/hypha.git
```

### Build and Test

Build the project:

```bash
cargo build
```

Run tests:

```bash
cargo test
```

Run code quality checks:

```bash
# Format code
cargo +nightly fmt

# Run linter
cargo clippy -- -D warnings
```

## Making Changes

### Create a Branch

Create a new branch for your changes:

```bash
git checkout -b feature/your-feature-name
```

Branch naming conventions:

- `feature/` - New features
- `fix/` - Bug fixes
- `docs/` - Documentation changes
- `refactor/` - Code refactoring
- `test/` - Test additions or changes

### Write Code

Follow the project's coding standards:

- **Clarity**: Write clear, readable code
- **Rust idioms**: Follow Rust best practices
- **Error handling**: Use `Result` types with meaningful errors
- **Documentation**: Document public APIs and complex logic
- **Tests**: Add tests for new functionality

### Commit Changes

Write clear commit messages following [Conventional Commits](https://www.conventionalcommits.org/):

```bash
git commit -m "feat(network): add automatic peer address registration

When an Identify event is received, add the peer's listen addresses
to the Kademlia routing table. This improves peer discovery.
"
```

Commit message format:

```
<type>(<scope>): <subject>

<body>

<footer>
```

Types:
- `feat`: New feature
- `fix`: Bug fix
- `docs`: Documentation changes
- `refactor`: Code refactoring
- `test`: Test additions or changes
- `chore`: Maintenance tasks

### Push Changes

Push your changes to your fork:

```bash
git push origin feature/your-feature-name
```

## Submitting Changes

### Create a Pull Request

1. Go to your fork on GitHub
2. Click "Pull Request"
3. Select your branch
4. Fill out the PR template
5. Submit the pull request

### PR Guidelines

A good pull request:

- **Has a clear description**: Explain what and why
- **References issues**: Link to related issues
- **Includes tests**: Test your changes
- **Passes CI**: All checks must pass
- **Is focused**: One feature or fix per PR
- **Has good commits**: Clear, logical commit history

### PR Review Process

1. **Automated checks**: CI runs tests and linters
2. **Code review**: Maintainers review your code
3. **Feedback**: Address any requested changes
4. **Approval**: PR is approved by maintainers
5. **Merge**: PR is merged into main branch

## Development Guidelines

### Code Style

Follow Rust formatting standards:

```bash
# Format your code
cargo +nightly fmt
```

### Linting

Fix clippy warnings:

```bash
# Check for issues
cargo clippy -- -D warnings

# Fix automatically (when possible)
cargo clippy --fix
```

### Testing

Write comprehensive tests:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_feature() {
        // Arrange
        let input = setup_test_data();

        // Act
        let result = my_function(input).await;

        // Assert
        assert_eq!(result, expected);
    }
}
```

### Documentation

Document public APIs:

```rust
/// Processes a task and returns the result.
///
/// # Arguments
///
/// * `task` - The task to process
///
/// # Returns
///
/// Returns `Ok(TaskResult)` on success, or `Err` if the task fails.
///
/// # Examples
///
/// ```
/// let task = Task::new("model-id", input);
/// let result = process_task(task).await?;
/// ```
pub async fn process_task(task: Task) -> Result<TaskResult> {
    // Implementation
}
```

### Anchor Comments

Use anchor comments for important context:

```rust
// NOTE: We use DialOpts to get a ConnectionId before the connection
// is fully established. This is crucial for tracking pending dials.
let opts = DialOpts::from(address);
let connection_id = opts.connection_id();
```

Anchor types:
- `NOTE:` - Important implementation details
- `TODO:` - Work that needs to be done
- `QUESTION:` - Uncertainties or design questions

## Working with Issues

### Finding Issues

Good first issues are tagged with:

- `good first issue` - Great for newcomers
- `help wanted` - Community contributions welcome
- `documentation` - Documentation improvements

### Reporting Bugs

When reporting bugs, include:

1. **Description**: Clear description of the issue
2. **Steps to reproduce**: How to reproduce the bug
3. **Expected behavior**: What should happen
4. **Actual behavior**: What actually happens
5. **Environment**: OS, Rust version, etc.
6. **Logs**: Relevant log output

### Suggesting Features

For feature requests, include:

1. **Use case**: Why is this needed?
2. **Proposed solution**: How should it work?
3. **Alternatives**: Other approaches considered
4. **Additional context**: Any relevant information

## Community

### Communication Channels

- **GitHub Issues**: Bug reports and feature requests
- **GitHub Discussions**: General questions and discussions
- **Pull Requests**: Code review and collaboration

### Getting Help

If you need help:

1. Check the [documentation](/getting-started/)
2. Search existing [GitHub issues](https://github.com/hypha-space/hypha/issues)
3. Ask in [GitHub Discussions](https://github.com/hypha-space/hypha/discussions)
4. Open a new issue with details

## Recognition

Contributors are recognized in:

- Release notes
- Contributors list
- Project documentation

Significant contributions may lead to becoming a project maintainer.

## License

By contributing to Hypha, you agree that your contributions will be licensed under the project's license.

## Questions?

If you have questions about contributing, please:

1. Check this guide
2. Review [CONTRIBUTING.md](https://github.com/hypha-space/hypha/blob/main/CONTRIBUTING.md)
3. Ask in GitHub Discussions
4. Open an issue

Thank you for contributing to Hypha!
