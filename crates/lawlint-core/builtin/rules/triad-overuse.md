---
id: triad-overuse
engine: statistical
scope: text
severity: warning
description: "Flags documents dense with three-part parallel constructions."
rationale: "One rule-of-three is rhetoric; a document-wide habit of triads is machine cadence. Unlike the per-span core/no-rule-of-three density rule, this measures the whole document's rate."
message: "Three-part constructions recur throughout the document; vary the structure."
metric: triad-density
threshold: 2
direction: above
examples:
  - bad: "The policy is clear, consistent, and fair. It protects employees, customers, and shareholders alike. Compliance requires training, monitoring, and enforcement. Each department must document, review, and certify its procedures. The board will assess progress quarterly, annually, and at each milestone. Managers must plan, supervise, and report their work. Auditors will test controls, trace transactions, and record exceptions. Counsel should identify risks, explain options, and recommend a course. The final report must be accurate, complete, and useful. The committee will review findings, assign owners, and track completion."
    good: "The policy is clear and fair. It protects employees and customers, and compliance requires training backed by real enforcement. Each department documents its procedures; the board assesses progress quarterly."
---
