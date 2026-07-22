---
id: no-weasel-attribution
engine: phrase
scope: text
severity: warning
description: "Flags vague attributions without a named source."
rationale: "Name the source or cut the unsupported claim."
message: "Name the source or cut the claim."
examples:
  - bad: "Experts agree that the policy will fail."
    good: "The 2024 Treasury report predicts the policy will fail."
patterns:
  - pattern: '(?i)\bexperts agree\b'
    message: "Name the source or cut the claim."
    suggestion: "Name the source or cut the claim."
  - pattern: '(?i)\bstudies show\b'
    message: "Name the source or cut the claim."
    suggestion: "Name the study or cut the claim."
  - pattern: '(?i)\bresearch (?:shows|suggests)\b'
    message: "Name the source or cut the claim."
    suggestion: "Name the research or cut the claim."
  - pattern: '(?i)\bmany argue\b'
    message: "Name the source or cut the claim."
    suggestion: "Name who argues this or cut the claim."
  - pattern: '(?i)\bwidely regarded as\b'
    message: "Name the source or cut the claim."
    suggestion: "Name who regards it this way."
  - pattern: '(?i)\bindustry reports suggest\b'
    message: "Name the source or cut the claim."
    suggestion: "Name the report or cut the claim."
  - pattern: '(?i)\bit is widely believed\b'
    message: "Name the source or cut the claim."
    suggestion: "Name who believes this or cut the claim."
---
