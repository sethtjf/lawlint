---
id: simple-tenses-only
engine: phrase
scope: prose
severity: warning
intent: style
description: Flags complex verb constructions outside the simple tenses.
rationale: Simple verb forms reduce ambiguity for technical readers. This conservative check targets explicit auxiliary patterns and avoids guessing at every past participle.
message: Prefer a simple tense or an adjective.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "The migration has completed and the table is being rebuilt."
    good: "The migration is complete. The system rebuilds the table."
patterns:
  - pattern: '(?i)\b(?:has|have|had)\s+been\s+\w+ing\b'
    message: "Replace the perfect progressive construction with a simple tense."
    suggestion: "Use a simple tense or an adjective."
  - pattern: '(?i)\b(?:is|are|was|were)\s+being\s+\w+ing\b'
    message: "Replace the progressive construction with a simple tense."
    suggestion: "Use a simple tense."
  - pattern: '(?i)\b(?:has|have|had)\s+(?:completed|finished|approved|started|stopped|changed|updated|installed|configured|failed|expired|occurred|increased|decreased|removed|created|deleted|closed|opened|received|sent|returned|provided|required|enabled|disabled)\b'
    message: "Replace the present or past perfect with a simple tense."
    suggestion: "Use a simple past or present tense."
  - pattern: '(?i)\b(?:is|are|was|were)\s+to\s+be\s+\w+(?:ed|en)\b'
    message: "Replace “is to be” with a direct instruction or simple tense."
    suggestion: "Use the imperative or a simple tense."
---
