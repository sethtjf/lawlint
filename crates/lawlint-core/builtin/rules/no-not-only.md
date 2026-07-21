---
id: no-not-only
engine: phrase
scope: text
severity: warning
description: "Flags not-only/but-also constructions"
rationale: "Avoid patterns that can make otherwise clear prose sound formulaic or overworked."
message: "Avoid the formulaic “not only ... but also” construction."
examples:
  - bad: "The ruling was not only wrong but also harmful."
    good: "The ruling was wrong and harmful."
patterns:
  - pattern: '(?is)\bnot only\b[\s\S]{0,120}\bbut also\b'
    message: "Avoid the formulaic “not only ... but also” construction."
---
