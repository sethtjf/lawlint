---
id: no-setup-payoff
engine: inferential
scope: text
severity: warning
description: "Flags staged questions or tensions resolved by the next sentence."
rationale: "A setup followed by a manufactured payoff delays a direct point for dramatic effect."
message: "State the substantive point without staging a question or tension."
examples:
  - bad: "What does the record show? It shows that the notice was late."
    good: "The record shows that the notice was late."
granularity: paragraph
---
Flag a paragraph when one sentence poses a rhetorical question, announces
a tension, or creates suspense solely so the next sentence can resolve it
with an obvious restatement. The setup and payoff should function as one
direct claim. Do not flag a genuine question that frames an issue the text
then answers with evidence, analysis, or a disputed legal standard. Do not
flag a transition that introduces a genuinely new section or issue.

## Flag examples
- "What does the record show? It shows that the notice was late."
- "The question is simple: who had notice? The answer is the agency."
- "This raises a serious point. The point is that the contract is unclear."
- "Why does that matter? It matters because the claim is untimely."

## Pass examples
- "What does the record show about delivery? The receipt is missing, and the witness disputes the date."
- "The question is whether the agency had notice before the deadline. The answer turns on the March 3 email."
- "The record raises a serious point: the contract uses two different dates for payment."
- "Why does that matter? Because the statute makes timely notice a condition of review."
