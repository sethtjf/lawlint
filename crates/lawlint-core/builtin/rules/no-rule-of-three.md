---
id: no-rule-of-three
engine: density
scope: text
severity: warning
description: "Flags dense repeated triplet constructions"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Repeated rule-of-three constructions can sound formulaic."
threshold: 12
examples:
  - bad: "The rule is clear, simple, and fair. The test is direct, narrow, and workable. The remedy is prompt, complete, and final. The record is old, incomplete, and unreliable. The briefing is careful, detailed, and extensive. The result is stable, predictable, and fair. The order is coherent, practical, and durable."
    good: "The rule is clear and fair."
patterns:
  - '(?i)\b\w+(?:\s+\w+){0,3},\s+\w+(?:\s+\w+){0,3},\s+and\s+\w+'
---
