---
id: no-metronomic-cadence
engine: statistical
scope: text
severity: suggestion
# Train-split check: cadence-autocorrelation above -0.306 fires on 133/165 AI
# and 125/165 human rows; train AUC 0.8719 is below the 0.9090 baseline.
intent: style
description: "Flags predictable sentence-length cadence"
message: "Sentence lengths follow a predictable cadence; vary the rhythm."
# The metric remains useful as a drafting signal, but not as an authorship
# signal.
metric: cadence-autocorrelation
threshold: -0.306
direction: above
examples:
  - bad: "The court reviewed the record. The parties filed their briefs today. The clerk entered the order after the hearing. Counsel must comply with the judgment. The court considered each disputed fact in the record. The parties may renew the motion after discovery. The clerk shall serve the order on counsel. Counsel shall comply with the order."
    good: "The court reviewed the motion. After extensive briefing, the record, including three depositions and two expert reports, still contains disputed facts that cannot be resolved on paper. Summary judgment is denied."
---
