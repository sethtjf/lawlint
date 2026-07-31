---
id: no-latin-abbreviations
engine: phrase
scope: prose
severity: warning
intent: style
description: Flags Latin abbreviations that can confuse technical readers.
rationale: Use plain English in place of Latin abbreviations, and name a complete list instead of using “etc.”
message: Replace the Latin abbreviation with plain English.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "Use a cache, e.g. Redis, i.e. a fast key-value store, etc."
    good: "Use a cache, for example Redis, that is, a fast key-value store."
patterns:
  - pattern: '(?i)\be\.g\.'
    message: "Write “for example”."
    suggestion: "Use “for example”."
    fix: "for example"
  - pattern: '(?i)\bi\.e\.'
    message: "Write “that is”."
    suggestion: "Use “that is”."
    fix: "that is"
  - pattern: '(?i)\betc\.'
    message: "Name the items instead of using “etc.”."
    suggestion: "Name the remaining items or write “and more”."
  - pattern: '(?i)\bviz\.'
    message: "Write “namely” or name the items."
    suggestion: "Use plain English."
  - pattern: '(?i)\bcf\.'
    message: "Write “compare” or state the comparison."
    suggestion: "Use plain English."
  - pattern: '(?i)\bn\.b\.'
    message: "Write “note” or state the information directly."
    suggestion: "Use plain English."
---
