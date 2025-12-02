---
name: User Story
about: Articulate a need from a user's perspective (Data Scientist, Operator, Developer)
title: "User Story: "
labels: 'user story'
assignees: ''

---

## Description

<!--
Describe the user need using the standard format below. This structure helps us understand who benefits and why.
-->

As a **[Role: e.g., Data Scientist, Node Operator, Platform Developer]**,
I want to **[Goal: e.g., monitor my training loss in real-time]**,
So that **[Benefit: e.g., I can stop failed runs early and save compute credits]**.

## Acceptance Criteria

<!--
Define what "done" looks like. Use checkboxes for each criterion so progress can be tracked.

Example:
- [ ] User can connect a TensorBoard instance to the worker node.
- [ ] Metrics are flushed to the logging endpoint every 10 seconds.
- [ ] The scheduler dashboard displays live loss curves for active jobs.
- [ ] Documentation is updated with the new monitoring workflow.
-->

## Technical Considerations

<!--
Share any constraints, dependencies, or Hypha-specific details that may affect implementation. This context helps developers make informed decisions.

Examples:
- "Must work over the existing libp2p circuit relay to avoid opening new ports."
- "Should respect mTLS requirements; metrics endpoints must be authenticated."
- "Needs to integrate with the existing `hypha-inspect probe` tooling."
- "Consider backward compatibility with workers running older versions."

Feel free to include links to relevant docs, RFCs, or related issues!
-->
