---
name: Task
about: A specific, assignable unit of work (dev, docs, or ops)
title: "Task: "
labels: 'task'
assignees: ''

---

## Description

<!--
Describe what specifically needs to be done. Be concrete and actionable.

Example: "Update the `hypha-inspect` CLI to support looking up peers by their public key, enabling operators to diagnose certificate mismatches without needing to know the full PeerId."
-->

## Context

<!--
Help others understand where this task fits in the bigger picture.

Examples:
- Parent epic: #123
- Related RFC: `rfc/2025-06-04_decentralized_task_announcement_protocol.md`
- Spawned from discussion in #456

Feel free to include any background that would help someone pick up this task!
-->

## Acceptance Criteria

<!--
Define what "done" looks like. Use checkboxes so progress can be tracked.

Example:
- [ ] CLI accepts `--pubkey` argument for secp256k1 keys.
- [ ] Output matches the existing format of `cert-info`.
- [ ] Unit tests covering the key conversion are passing.
- [ ] `--help` text is updated with the new option.
-->

## Implementation Hints (Optional)

<!--
Share any pointers that would help someone get started quickly. This section is optional but appreciated!

Examples:
- Relevant code: `crates/inspect/src/main.rs`
- Useful library: `libp2p::identity::Keypair::from_protobuf_encoding`
- Similar pattern: See how `--peer-id` lookup works in the same file
- Tip: The `hypha-crypto` crate has helpers for key format conversion

Don't worry if you're not sure, implementers can always ask questions!
-->
