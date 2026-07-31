---
id: approved-modals
engine: phrase
scope: prose
severity: warning
intent: style
description: Flags modal verbs that can make technical instructions uncertain.
rationale: Use can, will, and must for clear possibility, prediction, and requirement. Replace other modals only when the intended meaning supports it.
message: Prefer an approved modal or state the requirement directly.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "The service should restart after the update, and the request might fail."
    good: "The service must restart after the update, and the request can fail."
patterns:
  - pattern: '(?i)\b(?:may|might|could)\b'
    message: "Use “can” for possibility or permission."
    suggestion: "Replace this modal with “can” when it has that meaning."
    fix: "can"
  - pattern: '(?i)\bshould\b'
    message: "Replace “should” with “must” for a requirement, or state the recommendation directly."
    suggestion: "Choose “must” only when this is a requirement."
  - pattern: '(?i)\bwould\b'
    message: "Replace a hypothetical “would” with a direct condition."
    suggestion: "Restructure the sentence with an explicit condition."
---
