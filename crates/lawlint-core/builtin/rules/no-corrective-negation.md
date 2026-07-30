---
id: no-corrective-negation
engine: phrase
scope: text
severity: warning
description: "Flags corrective-negation constructions"
rationale: "State the substantive point without staging a correction."
message: "State the substantive point directly."
examples:
  - bad: "It's not that the rule is unclear, it's that the record is incomplete."
    good: "The record is incomplete."
patterns:
  - pattern: '(?i)\bit[''’]s\s+not\s+that\b[^.?!]{1,120},\s*it[''’]s\b'
    message: "Avoid the corrective-negation setup."
    suggestion: "State the second point directly."
  - pattern: '(?i)\bthis\s+isn[''’]t\s+about\b[^.?!]{1,120}\.\s*it[''’]s\b'
    message: "Avoid the corrective-negation setup."
    suggestion: "State the substantive point directly."
  - pattern: '(?i)\bnot\s+because\b[^.?!]{1,100}\bbut\s+because\b'
    message: "Avoid the corrective-negation setup."
    suggestion: "State the actual reason directly."
  - pattern: '(?i)\bless\s+about\b[^.?!]{1,100},\s*more\s+about\b'
    message: "Avoid the corrective-negation setup."
    suggestion: "State what the point is about."
---
