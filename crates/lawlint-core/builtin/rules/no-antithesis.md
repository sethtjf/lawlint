---
id: no-antithesis
engine: inferential
scope: text
severity: warning
description: "Flags rhetorical antithesis that balances one idea against another."
rationale: "Balanced opposition can make prose sound staged when it replaces a direct statement with a memorable contrast."
message: "State the point directly instead of staging a rhetorical opposition."
examples:
  - bad: "It is not the tool, but the workflow, that determines the result."
    good: "The workflow determines the result."
granularity: sentence
---
Flag a sentence when it uses balanced opposition as a rhetorical move:
"it is X, not Y", "less A, more B", "not X but Y", or a similar
parallel contrast whose main purpose is emphasis rather than precision.
Do not flag a contrast that carries substantive information, such as
distinguishing two legal standards, elements, causes, or remedies. Do not
flag an ordinary "but" clause that simply qualifies or limits a claim.

## Flag examples
- "It is not the tool, but the workflow, that determines the result."
- "The issue is less about the text and more about the remedy."
- "Not the deadline but the notice controls the analysis."
- "This is not a question of power; it is a question of judgment."

## Pass examples
- "The statute applies to employees but not to independent contractors."
- "The defendant challenges causation, but the record also raises a notice issue."
- "The rule distinguishes direct damages from consequential damages."
- "The motion is timely under Rule 12, but the court lacks personal jurisdiction."
