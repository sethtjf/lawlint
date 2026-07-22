---
id: no-superficial-analysis
engine: phrase
scope: text
severity: warning
description: "Flags trailing participial clauses that pretend to explain significance."
rationale: "Replace vague analysis with the concrete consequence or reason."
message: "Replace the vague analysis with a concrete consequence."
examples:
  - bad: "The launch adds file search, highlighting the team's commitment to better workflows."
    good: "The launch adds file search, so users can find old drafts without leaving the editor."
patterns:
  - pattern: '(?i),\s*(?:highlighting|underscoring|reflecting|showcasing|signaling|demonstrating|emphasizing|illustrating)\b\s+(?:the|its|their|a|how)\b'
    message: "Replace the vague analysis with a concrete consequence."
    suggestion: "State what the change does or why it matters."
---
