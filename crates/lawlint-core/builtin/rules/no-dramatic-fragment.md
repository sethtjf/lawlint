---
id: no-dramatic-fragment
engine: phrase
scope: text
severity: warning
description: "Flags dramatic fragments and negative-listing constructions."
rationale: "Use complete sentences and state the conclusion without staged fragments."
message: "Replace the dramatic fragment with a direct sentence."
examples:
  - bad: "Not a bug. Not a feature. A policy choice."
    good: "This is a policy choice."
patterns:
  - pattern: '(?i)\bthat[''’]s it\.\s*that[''’]s the\b'
    message: "Replace the dramatic fragment with a direct sentence."
    suggestion: "State the conclusion in one sentence."
  - pattern: '(?i)\bnot (?:a|an) [^.?!]{1,40}\.\s*not (?:a|an)\b'
    message: "Avoid negative-listing fragments."
    suggestion: "State what the subject is."
  - pattern: '(?i)\bfull stop\.'
    message: "Cut the dramatic fragment."
    suggestion: "End the sentence without emphasis."
---
