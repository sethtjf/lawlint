---
id: empty-hedge
engine: inferential
scope: text
severity: warning
description: "Flags hedges that carry no information about actual uncertainty."
rationale: "A hedge earns its place only when it tells the reader what is uncertain and why. Stacked qualifiers with no stated reason are AI filler."
message: "This hedge carries no information about actual uncertainty."
examples:
  - bad: "It might be said that the contract is, in some sense, ambiguous."
    good: "The contract is ambiguous because clause 4 conflicts with clause 9."
granularity: sentence
---
Flag a sentence when it hedges a claim without saying what is uncertain
or why. Empty hedges sound like: "it could perhaps be argued that",
"it might be said that", "to some extent", "in some sense", "arguably"
with no reason given, or several qualifiers stacked on one claim.
Do not flag a sentence that names a concrete source of uncertainty
(missing evidence, a pending ruling, ongoing treatment, a disputed fact)
or that states a likelihood together with the reason for it. Do not flag
a plain unhedged claim.

## Flag examples
- "It could perhaps be argued that the defendant bears some responsibility for the delay."
- "To some extent, the outcome may depend on various factors."
- "It might be said that the contract is, in some sense, ambiguous."
- "Arguably, there could be certain issues with the evidence presented."

## Pass examples
- "Damages are uncertain because the plaintiff's treatment is ongoing."
- "The court will likely deny the motion; three circuits have rejected identical arguments."
- "We cannot yet value the claim because the appraisal is not complete."
- "The defendant bears responsibility for the delay."
