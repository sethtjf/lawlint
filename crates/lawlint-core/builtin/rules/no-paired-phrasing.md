---
id: no-paired-phrasing
engine: statistical
scope: text
severity: warning
description: "Flags excessive paired phrasing"
message: "Coordinate and contrasting pairs occur too often; vary the phrasing."
# Train grid search: paired-adjective-rate above 4.124; train AUC was 0.8974
# for the added flag (base AUC 0.9016).
metric: paired-adjective-rate
threshold: 4.124
direction: above
examples:
  - bad: "The court sought clear and consistent rules for the parties. The parties offered careful and measured arguments about the record. The order should be fair and practical for everyone involved. The briefing presented precise and detailed objections to the proposed remedy. The witnesses gave calm and credible accounts during the hearing."
    good: "The court sought clear rules for the parties. The parties offered measured arguments about the record. The order should work in practice. The briefing presented detailed objections to the proposed remedy. The witnesses gave credible accounts during the hearing."
---
