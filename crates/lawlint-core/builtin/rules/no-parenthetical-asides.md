---
id: no-parenthetical-asides
engine: density
scope: text
severity: warning
intent: style
description: "Flags frequent parenthetical asides"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Parenthetical asides appear frequently; integrate important clauses into the sentence."
threshold: 15
examples:
  - bad: "The court (again) delayed (twice)."
    good: "The court delayed again, twice."
  - bad: "The remedy (which the court ordered) failed."
    good: "Under Section 4(b), the court-ordered remedy failed."
# A paren group attached directly to the preceding token is a statutory
# subdivision reference (Section 4(b), 12(a)(1)), not an aside — only count
# groups that follow whitespace or start the text. The capture group marks
# the aside itself; the consumed leading whitespace must not enter the span.
patterns:
  - '(?:^|\s)(\([^)]*\))'
---
