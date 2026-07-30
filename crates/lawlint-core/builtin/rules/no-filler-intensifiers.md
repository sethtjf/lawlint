---
id: no-filler-intensifiers
engine: phrase
scope: text
severity: error
# Train-split check: fires on human prose at a comparable rate (13/222 human,
# 15/222 AI); style lint, not an authorship signal.
intent: style
description: "Flags filler intensifiers"
rationale: "Remove intensifiers that add emphasis without adding evidence."
message: "Remove the filler intensifier."
examples:
  - bad: "The result is genuinely important."
    good: "The result is important."
patterns:
  - pattern: '(?i)\b(?:genuinely|really|truly)\b'
    message: "Remove the filler intensifier."
    suggestion: "Delete the intensifier or state the concrete degree."
  - pattern: '(?i)\bactually\b'
    message: "Remove the filler intensifier."
    suggestion: "Delete “actually” unless it marks a legally relevant fact."
allow_context: { pattern: '(?i)\bactually\s+(?:incurred|knew|received|paid|suffered|occurred)\b', window: 48 }
---
