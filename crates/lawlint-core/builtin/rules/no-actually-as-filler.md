---
id: no-actually-as-filler
engine: phrase
scope: text
severity: warning
# Train-split check: the legal-context exception keeps “actually incurred”
# and similar uses clear of this style lint.
intent: style
description: "Flags “actually” when it adds emphasis instead of a fact."
rationale: "Use “actually” only when it marks a legally relevant fact or correction."
message: "Remove “actually” unless it marks a legally relevant fact."
examples:
  - bad: "The claim is actually weak."
    good: "The claim is weak."
patterns:
  - pattern: '(?i)\bactually\b'
    message: "Remove “actually” unless it marks a legally relevant fact."
    suggestion: "Delete “actually” unless it marks a legally relevant fact."
allow_context: { pattern: '(?i)\bactually\s+(?:incurred|knew|received|paid|suffered|occurred)\b', window: 48 }
---
