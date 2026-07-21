---
id: no-empty-emphasis
engine: density
scope: text
severity: warning
description: "Flags overused empty emphasis words"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Replace emphasis with a specific fact or omit it."
threshold: 12
examples:
  - bad: "This is very significantly important."
    good: "This raises damages by forty percent."
patterns:
  - '(?i)\b(?:very|really|significantly|crucially)\b'
---
