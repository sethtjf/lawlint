---
id: no-anaphoric-openers
engine: statistical
scope: text
severity: warning
# Train-split check: repeated-opener-density above 0.211 fires on 30/165 AI
# and 12/165 human rows; train AUC 0.8831 is below the 0.9090 baseline.
intent: style
description: "Flags repeated sentence openers across a document"
message: "Sentence openers repeat too often across the document; vary the openings."
metric: repeated-opener-density
threshold: 0.211
direction: above
examples:
  - bad: "The court reviewed the motion and the supporting record. The court considered the response from both parties. The court examined the affidavits submitted with the motion. The court compared those affidavits with the hearing transcript. The court rejected the unsupported account. The court denied relief on the present record. The court entered judgment for the respondent. The court retained jurisdiction over costs."
    good: "The court reviewed the motion and the supporting record. It considered the response from both parties, compared the affidavits with the hearing transcript, and rejected the unsupported account. Relief was denied on the present record, and the court retained jurisdiction over costs."
---
