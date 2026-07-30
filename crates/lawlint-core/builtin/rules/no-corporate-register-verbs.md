---
id: no-corporate-register-verbs
engine: phrase
scope: text
severity: warning
description: "Flags corporate-register verbs"
rationale: "Prefer concrete verbs over corporate language that inflates ordinary actions."
message: "Use a concrete verb instead of corporate-register language."
examples:
  - bad: "The report showcases the team's capabilities."
    good: "The report shows what the team can do."
patterns:
  - pattern: '(?i)\b(?:underscore|showcase(?:s|d)?|spotlight(?:s|ed)?|utilize(?:s|d)?|operationalize(?:s|d)?|incentivize(?:s|d)?)\b'
    message: "Use a concrete verb instead of corporate-register language."
    suggestion: "Replace it with a direct verb such as “show” or “use”."
  - pattern: '(?i)\breflect(?:s|ed|ing)?\s+(?:the\s+)?(?:commitment|importance|complexity|need|significance)\b'
    message: "Avoid corporate-register uses of “reflect”."
    suggestion: "State the concrete fact or consequence."
---
