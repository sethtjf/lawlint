---
id: prefer-spoken-voice
engine: inferential
scope: text
severity: warning
intent: style
description: "Flags prose that works on the page but not in a spoken explanation."
rationale: "Clear legal writing can remain precise while using sentences a reader could say aloud."
message: "Rewrite this paragraph in precise language that would also sound natural aloud."
examples:
  - bad: "The foregoing analysis is dispositive with respect to the aforementioned issue in light of the fact that the parties' respective positions are mutually inconsistent."
    good: "That analysis resolves the issue because the parties take conflicting positions."
granularity: paragraph
---
Flag a paragraph when its meaning depends on clause pile-ups, stacked
abstractions, or formal connective phrases that would be difficult to say
and follow aloud. Consider the whole paragraph, not one technical term.
Do not flag precise legal language merely because it is formal. Do not flag
necessary terms of art, statutory language, citations, or a dense sentence
that remains clear when read aloud.

## Flag examples
- "The foregoing analysis is dispositive with respect to the aforementioned issue in light of the fact that the parties' respective positions are mutually inconsistent."
- "Notwithstanding the foregoing, the implementation of the contemplated remediation methodology remains subject to further consideration by the appropriate authority."
- "The determination of whether the provision is applicable necessitates an evaluation of the totality of the circumstances in their aggregate."
- "In connection with the issue hereinabove discussed, the undersigned respectfully submits that the requested relief is warranted."

## Pass examples
- "That analysis resolves the issue because the parties take conflicting positions."
- "The agency must decide whether the proposed remedy complies with the statute."
- "The court considers the totality of the circumstances."
- "The brief asks the court to dismiss the claim under Rule 12(b)(6)."
