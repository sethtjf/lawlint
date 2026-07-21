---
id: no-marketing-language
engine: phrase
scope: text
severity: error
description: "Flags marketing language, hype, and filler."
rationale: "Avoid patterns that can make otherwise clear prose sound formulaic or overworked."
message: "Avoid marketing language, hype, and filler."
examples:
  - bad: "We leverage a robust platform."
    good: "We rely on a dependable platform."
patterns:
  - pattern: '(?i)\bleverage\b'
    message: "Avoid the marketing verb “leverage”."
    suggestion: "Use “use” or a concrete verb."
  - pattern: '(?i)\bunlock\b'
    message: "Avoid the hype verb “unlock”."
    suggestion: "Describe the actual outcome."
  - pattern: '(?i)\bpowerful\b'
    message: "Avoid the filler adjective “powerful”."
    suggestion: "State the specific capability."
  - pattern: '(?i)\bseamless(?:ly)?\b'
    message: "Avoid the hype word “seamless”."
    suggestion: "Describe what actually happens."
  - pattern: '(?i)\brobust\b'
    message: "Avoid the filler adjective “robust”."
    suggestion: "Name the concrete property."
  - pattern: '(?i)\bcutting[- ]edge\b'
    message: "Avoid the hype phrase “cutting-edge”."
    suggestion: "Say what it is."
  - pattern: '(?i)\bdelve\b'
    message: "Avoid “delve”."
    suggestion: "Use a direct verb such as “examine”."
  - pattern: '(?i)\btapestry\b'
    message: "Avoid the metaphor “tapestry”."
  - pattern: '(?i)\bin the realm of\b'
    message: "Avoid “in the realm of”."
    suggestion: "Name the subject directly."
  - pattern: '(?i)\bnavigate the landscape of\b'
    message: "Avoid “navigate the landscape of”."
    suggestion: "Describe the task."
  - pattern: '(?i)\bit[''’]s worth noting that\b'
    message: "State the point directly."
    suggestion: "Drop the throat-clearing."
  - pattern: '(?i)\bat the end of the day\b'
    message: "Avoid the filler phrase “at the end of the day”."
---
