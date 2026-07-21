---
id: sentence-length
engine: statistical
scope: text
severity: warning
intent: style
description: "Flags sentences that are difficult to read."
message: "Sentence is too long; consider shortening it."
metric: sentence-length
params: { max_words: 45 }
examples:
  - bad: "The court, having reviewed the parties' extensive submissions and the voluminous record developed over three years of contentious litigation, and having heard four hours of oral argument on the competing motions for summary judgment, concludes that genuine disputes of material fact preclude judgment for either side at this stage of the proceedings."
    good: "The court reviewed the record. Fact disputes preclude summary judgment."
---
