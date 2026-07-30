---
id: no-hedging
engine: density
scope: text
severity: warning
description: "Flags excessive hedging language"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Reduce hedging and make the claim more direct."
threshold: 10
examples:
  - bad: "Perhaps the claim is arguably strong."
    good: "The claim is strong."
patterns:
  - '(?i)\b(?:arguably|it could be said|generally speaking|perhaps|likely|possibly|somewhat|relatively|fairly|rather|to some extent|in many ways|tends to|may well)\b'
---
