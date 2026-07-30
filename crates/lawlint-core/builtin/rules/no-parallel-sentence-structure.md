---
id: no-parallel-sentence-structure
engine: inferential
scope: text
severity: warning
description: "Flags repeated syntactic templates across sentences in a paragraph."
rationale: "Repeated sentence templates can make a paragraph sound metronomic even when its words change."
message: "Vary the sentence structure within this paragraph."
examples:
  - bad: "The court reviewed the motion. The court considered the response. The court rejected the argument."
    good: "After reviewing the motion and response, the court rejected the argument."
granularity: paragraph
---
Flag a paragraph when two or more sentences rely on the same syntactic
template for emphasis, such as repeated subject-verb openings, repeated
"the X is..." clauses, or repeated imperative forms. The repetition must
shape the paragraph's rhythm, not merely result from a shared legal term.
Do not flag parallelism inside one enumerated list, a statutory recitation,
a quoted provision, or another context where the repeated structure is
correct and functional. Do not flag ordinary repetition of a party's name
or a defined term when the surrounding sentence structures differ.

## Flag examples
- "The court reviewed the motion. The court considered the response. The court rejected the argument."
- "The agency found that the report was late. The agency found that the notice was incomplete. The agency found that the appeal was barred."
- "The contract requires notice. The contract requires payment. The contract requires arbitration."
- "The witness said she saw the car. The witness said she heard the impact. The witness said she called 911."

## Pass examples
- "After reviewing the motion and response, the court rejected the argument."
- "The agency found that the report was late, concluded that notice was incomplete, and dismissed the appeal."
- "The contract requires notice and payment; it also requires arbitration."
- "The witness saw the car, heard the impact, and called 911."
