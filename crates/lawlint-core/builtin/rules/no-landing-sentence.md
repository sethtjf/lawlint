---
id: no-landing-sentence
engine: inferential
scope: text
severity: warning
# Judged train smoke (20 AI / 20 human): flagged 0 AI and 0 human rows;
# unmeasured support, so this remains a style lint.
intent: style
description: "Flags short closing sentences written for rhetorical effect."
rationale: "A punchy closer can perform finality without stating a holding or consequence."
message: "Replace the rhetorical closer with the concrete holding or consequence."
examples:
  - bad: "The record contains no evidence of delivery. That's the whole game."
    good: "The record contains no evidence of delivery. The notice was therefore ineffective."
granularity: paragraph
---
Flag a short final sentence written mainly to create a punchy sense of
finality, such as "that's the whole game", "nothing else matters", or
"end of story". The sentence should be rhetorical rather than informative.
Do not flag a short sentence that states a real holding, amount, deadline,
or other material fact. Do not flag a concise conclusion merely because it
ends a paragraph.

## Flag examples
- "The record contains no evidence of delivery. That's the whole game."
- "The agency never made the required finding. Nothing else matters."
- "The clause has no exception. End of story."
- "The witness changed her account three times. That settles it."

## Pass examples
- "The record contains no evidence of delivery. The notice was therefore ineffective."
- "The agency never made the required finding. The order must be vacated."
- "The clause has no exception. It applies to this claim."
- "The witness changed her account three times. The court found her testimony unreliable."
