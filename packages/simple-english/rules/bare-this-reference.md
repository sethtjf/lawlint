---
id: bare-this-reference
engine: phrase
scope: prose
severity: suggestion
intent: style
description: Flags sentence-initial “this” without a following noun.
rationale: A noun after “this” gives the reader a clear referent and reduces ambiguity.
message: Add a noun after “this”.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "This shows that the cache is stale."
    good: "This result shows that the cache is stale."
patterns:
  - pattern: '(?i)(?:^|[.!?]\s+)this\s+(?:is|was|means|allows|makes|shows|can|will)\b'
    message: "Add a noun after “this”."
    suggestion: "Write “this result”, “this setting”, or another specific noun."
---
