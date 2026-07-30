---
id: no-em-dash
engine: phrase
scope: text
severity: error
# Train-split check: fires on human prose more often than AI prose (44/222
# human, 5/222 AI); typography lint, not an authorship signal.
intent: style
description: "Flags em dashes"
rationale: "Use commas, parentheses, or a new sentence instead of an em dash."
message: "Avoid em dashes."
examples:
  - bad: "The court—wisely—paused."
    good: "The court wisely paused."
patterns:
  - pattern: '—'
    message: "Avoid em dashes."
    suggestion: "Use a comma, parentheses, or a new sentence."
---
