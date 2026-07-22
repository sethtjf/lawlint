---
id: no-importance-puffery
engine: phrase
scope: text
severity: error
description: "Flags inflated claims about importance or status."
rationale: "State the fact and let the reader judge whether it matters."
message: "State the fact without importance puffery."
examples:
  - bad: "The launch marks a pivotal moment for the company."
    good: "The launch is the company's first paid product."
patterns:
  - pattern: '(?i)\bstands as a testament\b'
    message: "Avoid importance puffery."
    suggestion: "State the concrete fact."
  - pattern: '(?i)\ba testament to\b'
    message: "Avoid importance puffery."
    suggestion: "State what happened."
  - pattern: '(?i)\bmarks a pivotal moment\b'
    message: "Avoid importance puffery."
    suggestion: "Name the concrete change."
  - pattern: '(?i)\bplays a (?:vital|crucial|pivotal) role\b'
    message: "Avoid importance puffery."
    suggestion: "Describe the actual function."
  - pattern: '(?i)\bsolidifies its position\b'
    message: "Avoid importance puffery."
    suggestion: "State the concrete result."
  - pattern: '(?i)\bunderscores (?:its significance|the importance)\b'
    message: "Avoid importance puffery."
    suggestion: "State the fact directly."
---
