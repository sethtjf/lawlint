---
id: no-performed-enthusiasm
engine: phrase
scope: text
severity: warning
description: "Flags performed enthusiasm"
rationale: "Use a factual description instead of performed affect."
message: "Replace performed enthusiasm with a concrete statement."
examples:
  - bad: "We are thrilled to announce the new filing system."
    good: "The new filing system is available."
patterns:
  - pattern: '(?i)\b(?:excited|thrilled)\s+to\b'
    message: "Avoid performed enthusiasm."
    suggestion: "State what happened or what is available."
  - pattern: '(?i)\bwe[''’]re\s+pumped\b'
    message: "Avoid performed enthusiasm."
    suggestion: "State the concrete result."
  - pattern: '(?i)\bcan[''’]t\s+wait\b'
    message: "Avoid performed enthusiasm."
    suggestion: "Give the relevant date or action."
  - pattern: '(?i)\blove\s+(?:this|that)\b'
    message: "Avoid performed enthusiasm."
    suggestion: "Describe the specific benefit."
---
