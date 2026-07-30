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
  - pattern: '(?i)(?:^|[.!?]\s+)no\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*[.!?]\s+no\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*'
    message: "Avoid repeated “No …” openers."
    suggestion: "Combine the points in a direct sentence."
  - pattern: '(?i)(?:^|[.!?]\s+)not\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*,\s*not\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*'
    message: "Avoid repeated “Not …” structure."
    suggestion: "State the positive point directly."
  - pattern: '(?i)(?:^|[.!?]\s+)never\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*;\s*never\s+(?:[a-z][a-z-]*\s+){0,4}[a-z][a-z-]*'
    message: "Avoid repeated “Never …” structure."
    suggestion: "State the rule or fact directly."
---
