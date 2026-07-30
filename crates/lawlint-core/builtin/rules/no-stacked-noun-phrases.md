---
id: no-stacked-noun-phrases
engine: density
scope: text
severity: warning
# Train-split density sweep: threshold 4 fires on 11/222 AI and 17/222 human
# rows; comparable human rate makes this a style lint.
intent: style
description: "Flags stacked noun phrases before abstract head nouns"
rationale: "Unpack dense noun stacks so the relationship between ideas is explicit."
message: "Unpack the stacked noun phrase."
threshold: 4
patterns:
  - '(?i)\b(?:[a-z][a-z-]+\s+){2,}(?:strategy|framework|process|solution|initiative|approach|capability|infrastructure|methodology|program)\b'
examples:
  - bad: "The agency adopted a long-term claims processing framework."
    good: "The agency adopted a framework for processing claims over the long term."
---
