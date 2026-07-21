---
id: no-semicolons
engine: phrase
scope: text
severity: error
intent: style
description: "Flags semicolons."
rationale: "Avoid patterns that can make otherwise clear prose sound formulaic or overworked."
message: "Prefer periods over semicolons."
examples:
  - bad: "The motion failed; the court adjourned."
    good: "The motion failed. The court adjourned."
patterns:
  - pattern: ';'
    message: "Prefer periods over semicolons."
    suggestion: "Two short sentences beat one stitched-together one."
---
