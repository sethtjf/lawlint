---
id: no-faux-insight
engine: phrase
scope: text
severity: warning
description: "Flags faux-insight setups that posture before making a claim."
rationale: "Make the claim stand on its own instead of flattering the writer as the lone expert."
message: "Cut the faux-insight setup and state the claim."
examples:
  - bad: "The part everyone misses: distribution is the real moat."
    good: "Distribution is the moat."
patterns:
  - pattern: '(?i)\bwhat most people get wrong\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bhere[''’]s what nobody tells you\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bthe part everyone misses\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bthis is the part most people skip\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bhere[''’]s the thing\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bhere[''’]s what i mean\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
  - pattern: '(?i)\bthe uncomfortable truth is\b'
    message: "Cut the faux-insight setup."
    suggestion: "State the claim directly."
---
