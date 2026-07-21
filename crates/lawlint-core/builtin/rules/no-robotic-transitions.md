---
id: no-robotic-transitions
engine: density
scope: text
severity: warning
description: "Flags overuse of formulaic transitions"
rationale: "Use this signal as a prompt to revise rhythm and density, not as a hard prohibition."
message: "Formulaic sentence transitions are overused."
threshold: 18
examples:
  - bad: "Moreover, the claim fails."
    good: "The claim also fails."
patterns:
  - '(?im)^\s*(Moreover|Furthermore|Additionally|In conclusion),'
---
