---
id: no-metronomic-cadence
engine: statistical
scope: text
severity: warning
description: "Flags predictable sentence-length cadence"
message: "Sentence lengths follow a predictable cadence; vary the rhythm."
# Train grid search: cadence-autocorrelation above -0.306; train AUC was
# 0.8719 for the added flag (base AUC 0.8922).
metric: cadence-autocorrelation
threshold: -0.306
direction: above
examples:
  - bad: "The court reviewed the record. The parties filed their briefs today. The clerk entered the order after the hearing. Counsel must comply with the judgment. The court considered each disputed fact in the record. The parties may renew the motion after discovery. The clerk shall serve the order on counsel. Counsel shall comply with the order."
    good: "The court reviewed the motion. After extensive briefing, the record—three depositions and two expert reports—still contains disputed facts that cannot be resolved on paper. Summary judgment is denied."
---
