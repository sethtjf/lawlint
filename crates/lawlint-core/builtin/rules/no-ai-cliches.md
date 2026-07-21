---
id: no-ai-cliches
engine: phrase
scope: text
severity: warning
description: "Flags common AI-writing clichés."
rationale: "Avoid patterns that can make otherwise clear prose sound formulaic or overworked."
message: "Avoid common AI-writing clichés."
examples:
  - bad: "We should delve into this issue."
    good: "We should examine this issue."
patterns:
  - pattern: '(?i)\bdelve\b'
    message: "Avoid the AI-writing cliché “delve”."
    suggestion: "Use a direct verb such as “examine”."
  - pattern: '(?i)\btapestry\b'
    message: "Avoid the metaphor “tapestry” in analytical prose."
  - pattern: '(?i)\blandscape of\b'
    message: "Avoid the vague phrase “landscape of”."
  - pattern: '(?i)\bin today''s fast-paced world\b'
    message: "Avoid this generic introductory phrase."
  - pattern: '(?i)\bit is important to note\b'
    message: "State the important point directly."
  - pattern: '(?i)\bnavigate the complexities\b'
    message: "Use a concrete description of the task or issue."
---
