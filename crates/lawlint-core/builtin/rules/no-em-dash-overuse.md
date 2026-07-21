---
id: no-em-dash-overuse
engine: density
scope: text
severity: warning
description: "Flags excessive em dashes"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Em dashes are used too frequently."
threshold: 8
examples:
  - bad: "The court—wisely—paused."
    good: "The court wisely paused."
patterns:
  - '—'
---
