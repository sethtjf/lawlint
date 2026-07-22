---
id: no-binary-contrast
engine: phrase
scope: text
severity: warning
description: "Flags formulaic binary-contrast constructions."
rationale: "State the substantive point directly instead of staging a false choice."
message: "State the substantive point directly."
examples:
  - bad: "The question isn't the model. It's the eval."
    good: "The eval matters more than the model."
patterns:
  - pattern: '(?i)\bthe (?:question|problem|issue|point) isn[''’]t\b[^.?!]{0,80}[.,;]\s*it[''’]s\b'
    message: "Avoid the binary-contrast setup."
    suggestion: "State the second point directly."
  - pattern: '(?i)\bit[''’]s not just\b[^.?!]{0,80}\bit[''’]s\b'
    message: "Avoid the binary-contrast setup."
    suggestion: "State the stronger point directly."
  - pattern: '(?i)\bthis is not\b[^.?!]{0,80}\.\s*it[''’]s\b'
    message: "Avoid the binary-contrast setup."
    suggestion: "State the second point directly."
---
