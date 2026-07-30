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
  - pattern: '(?i)\bmake no mistake\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the point directly."
  - pattern: '(?i)\bto be clear\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the clarification directly."
  - pattern: '(?i)\blet[''’]s be honest\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the relevant fact directly."
  - pattern: '(?i)\bthe reality is\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the reality directly."
  - pattern: '(?i)\bhere[''’]s the reality\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the point directly."
  - pattern: '(?i)\band that[''’]s the point\b'
    message: "Cut the rhetorical setup."
    suggestion: "State the point directly."
---
