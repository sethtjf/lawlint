---
id: no-colon-reveal
engine: leading
scope: text
severity: warning
description: "Flags dramatic colon reveals at sentence starts."
rationale: "Use colons for lists, labels, and quotes, not staged revelations."
message: "Replace the dramatic colon reveal with a direct sentence."
examples:
  - bad: "The best part: it learns from mistakes."
    good: "It learns from mistakes."
patterns:
  - pattern: 'the best part\s*:'
    message: "Replace the dramatic colon reveal with a direct sentence."
    suggestion: "State the following point directly."
  - pattern: 'the kicker\s*:'
    message: "Replace the dramatic colon reveal with a direct sentence."
    suggestion: "State the following point directly."
  - pattern: 'the catch\s*:'
    message: "Replace the dramatic colon reveal with a direct sentence."
    suggestion: "State the following point directly."
  - pattern: 'the result\s*:'
    message: "Replace the dramatic colon reveal with a direct sentence."
    suggestion: "State the following point directly."
---
