---
id: no-passive-overuse
engine: density
scope: text
severity: warning
description: "Flags likely passive-voice overuse"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Passive voice appears frequently; prefer active constructions where possible."
threshold: 25
examples:
  - bad: "The motion was denied by the court."
    good: "The court denied the motion."
patterns:
  - '(?i)\b(?:is|are|was|were|be|been|being)\s+\w+(?:ed|en)\b'
---
