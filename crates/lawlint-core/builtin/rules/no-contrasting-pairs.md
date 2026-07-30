---
id: no-contrasting-pairs
engine: phrase
scope: text
severity: warning
description: "Flags formulaic contrasting pairs"
rationale: "State the quality or limitation directly instead of balancing paired opposites."
message: "State the point directly instead of using a contrasting pair."
examples:
  - bad: "The analysis should be clear without becoming simplistic, balanced without becoming vague."
    good: "The analysis should be clear and balanced."
patterns:
  - pattern: '(?i)\b\w+(?:\s+\w+){0,5}\s+without\s+(?:becoming|sounding)\s+\w+|\b\w+(?:\s+\w+){0,5}\s+without\s+being\s+(?:too|overly|unduly|excessively)\s+\w+'
    message: "Avoid the formulaic contrasting pair."
    suggestion: "State the quality and its limit directly."
---
