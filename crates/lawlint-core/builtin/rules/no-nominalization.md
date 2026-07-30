---
id: no-nominalization
engine: density
scope: text
severity: warning
# Train-split density sweep: threshold 1 fires on 0/222 AI and 9/222 human
# rows; style lint because this construction is more common in human prose.
intent: style
description: "Flags weak verbs paired with nominalized nouns"
rationale: "Use the plain verb when it makes the sentence shorter and clearer."
message: "Prefer the plain verb to a weak verb plus nominalized noun."
threshold: 1
examples:
  - bad: "The court made a determination about the claim."
    good: "The court determined the claim."
patterns:
  - '(?i)\b(?:make|made|makes|provide|provided|provides|conduct|conducted|conducts|perform|performed|performs|carry\s+out|carried\s+out|reach|reached|undertake|undertook)\s+(?:a|an|the)?\s*(?:\w+\s+){0,2}(?:determination|explanation|assessment|evaluation|implementation|investigation|analysis|decision|consideration|recommendation|conversation|discussion)\b'
---
