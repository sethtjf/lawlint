---
id: no-negative-parallelism
engine: phrase
scope: text
severity: warning
description: "Flags repeated negative parallelism"
rationale: "Use a direct statement instead of a chain of negative fragments."
message: "Replace the repeated negative structure with a direct statement."
examples:
  - bad: "No delay. No explanation. No remedy."
    good: "The record shows no delay, explanation, or remedy."
patterns:
  - pattern: '(?i)\bno\s+[^.!?]{1,70}[.!?]\s*no\s+'
    message: "Avoid repeated “No …” openers."
    suggestion: "Combine the points in a direct sentence."
  - pattern: '(?i)\bnot\s+[^,.;!?]{1,45},\s*not\s+'
    message: "Avoid repeated “Not …” structure."
    suggestion: "State the positive point directly."
  - pattern: '(?i)\bnever\s+[^;.!?]{1,55};\s*never\s+'
    message: "Avoid repeated “Never …” structure."
    suggestion: "State the rule or fact directly."
allow_context:
  pattern: '(?i)\b(?:semicolons|dashes|doublets|moreover|markdown|gap\s+for\s+interpretation)\b'
  window: 80
---
