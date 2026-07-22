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
  - pattern: '(?i)\bfoster\b'
    message: "Avoid the vague verb “foster”."
    suggestion: "Name the action or result."
  - pattern: '(?i)\butilize\b'
    message: "Prefer the plain verb “use”."
    suggestion: "Use “use”."
  - pattern: '(?i)\bfacilitate\b'
    message: "Avoid the vague verb “facilitate”."
    suggestion: "Name what the subject does."
  - pattern: '(?i)\bempower\b'
    message: "Avoid the vague verb “empower”."
    suggestion: "State what people can do."
  - pattern: '(?i)\bstreamline\b'
    message: "Avoid the vague verb “streamline”."
    suggestion: "Name the step or delay removed."
  - pattern: '(?i)\bparadigm shift\b'
    message: "Avoid the hype phrase “paradigm shift”."
    suggestion: "Describe the actual change."
  - pattern: '(?i)\bgame[- ]changer\b'
    message: "Avoid the hype phrase “game changer”."
    suggestion: "Describe the concrete effect."
  - pattern: '(?i)\bthis is huge\b'
    message: "Avoid the hype phrase “this is huge”."
    suggestion: "State what changed and why it matters."
  - pattern: '(?i)\bthis changes everything\b'
    message: "Avoid the hype phrase “this changes everything”."
    suggestion: "Describe the specific consequence."
  - pattern: '(?i)\bbeacon\b'
    message: "Avoid the vague metaphor “beacon”."
    suggestion: "State what the subject demonstrates."
  - pattern: '(?i)\bmulti[- ]faceted\b'
    message: "Avoid the vague adjective “multifaceted”."
    suggestion: "Name the relevant parts."
  - pattern: '(?i)\bmeticulous(?:ly)?\b'
    message: "Avoid the praise word “meticulous”."
    suggestion: "Describe what was checked."
  - pattern: '(?i)\bintricate\b'
    message: "Avoid the vague adjective “intricate”."
    suggestion: "Explain the actual complexity."
  - pattern: '(?i)\bparamount\b'
    message: "Avoid the inflated adjective “paramount”."
    suggestion: "Say what is most important."
  - pattern: '(?i)\btransformative\b'
    message: "Avoid the hype adjective “transformative”."
    suggestion: "Describe the measurable change."
  - pattern: '(?i)\belevate\b'
    message: "Avoid the vague verb “elevate”."
    suggestion: "Name the improvement."
  - pattern: '(?i)\bembark(?:ing)?\s+on\b'
    message: "Avoid the inflated phrase “embark on”."
    suggestion: "Use “begin” or name the action."
  - pattern: '(?i)\bsupercharge\b'
    message: "Avoid the hype verb “supercharge”."
    suggestion: "Describe the actual improvement."
  - pattern: '(?i)\bharness\b'
    message: "Avoid the vague verb “harness”."
    suggestion: "Use a concrete verb such as “use”."
  - pattern: '(?i)\bever[- ]evolving\b'
    message: "Avoid the vague phrase “ever-evolving”."
    suggestion: "Name what changed."
---
