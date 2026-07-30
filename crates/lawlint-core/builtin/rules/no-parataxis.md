---
id: no-parataxis
engine: inferential
scope: text
severity: warning
description: "Flags marches of short coordinate clauses and sentences."
rationale: "A string of equal short clauses can create a mechanical rhythm when the relationships between ideas deserve subordination."
message: "Join or subordinate these short clauses so their relationship is clear."
examples:
  - bad: "The court read the contract. It reviewed the emails. It considered the testimony. It denied the motion."
    good: "After reviewing the contract, emails, and testimony, the court denied the motion."
granularity: paragraph
---
Flag a paragraph when it marches through several short coordinate clauses
or sentences, especially repeated "and" clauses, without showing which idea
causes, limits, or supports another. Look for a mechanical sequence rather
than a single concise sentence. Do not flag deliberately short holdings,
operative commands, or citation sentences when their brevity is normal for
legal prose. Do not flag a paragraph whose short sentences each carry a
distinct legal step or whose coordination makes the relationship clear.

## Flag examples
- "The court reviewed the motion. It read the response. It checked the docket. It set a hearing."
- "The witness arrived and she sat and she waited and she testified."
- "The agency issued the rule. It took comments. It changed the text. It published the rule."
- "The contract says payment is due. It sets a date. It states a place. It gives a remedy."

## Pass examples
- "The court denied the motion because the response was untimely."
- "The court held that the notice was late. The judgment followed."
- "The clerk entered judgment. See Fed. R. Civ. P. 58."
- "The agency issued the rule, accepted comments, revised the text, and published the final version."
