---
id: no-rhetorical-setups
engine: phrase
scope: text
severity: warning
description: "Flags rhetorical setups that delay the substantive point."
rationale: "Make the point directly instead of staging a reveal for the reader."
message: "Cut the rhetorical setup and state the point."
examples:
  - bad: "What if I told you the filing deadline already passed?"
    good: "The filing deadline already passed."
patterns:
  - pattern: '(?i)\bwhat if i told you\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the point directly."
  - pattern: '(?i)\bthink about it\s*:'
    message: "Cut the rhetorical setup."
    suggestion: "State the implication directly."
  - pattern: '(?i)\bplot twist\s*:'
    message: "Cut the rhetorical setup."
    suggestion: "State the twist as a fact."
  - pattern: '(?i)\blet that sink in\b'
    message: "Cut the rhetorical setup."
    suggestion: "End with the point."
  - pattern: '(?i)\bread that again\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the point once."
---
