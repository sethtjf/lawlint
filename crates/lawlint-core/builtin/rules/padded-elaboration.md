---
id: padded-elaboration
engine: inferential
scope: text
severity: warning
description: "Flags sentences that restate the previous point without adding new information."
rationale: "Restating a point in different words pads the text without informing the reader. Every sentence should add a fact, reason, example, or consequence."
message: "This sentence restates the previous point without adding new information."
examples:
  - bad: "The statute of limitations has expired. In other words, the deadline for filing has passed."
    good: "The statute of limitations has expired. The plaintiff filed on March 3, sixty days late."
granularity: paragraph
---
Flag a sentence or clause that repeats the point of the sentence before it
in different words while adding no new fact, reason, number, example,
exception, or consequence. Padding often starts with "in other words",
"that is to say", "put simply", "this means that", or "essentially",
and then says the same thing again. Do not flag a follow-up sentence
that adds something new: a date, an amount, an example, a cause, an
exception, or what happens next.

## Flag examples
- "The statute of limitations has expired. In other words, the deadline for filing has passed."
- "The contract is unenforceable. Put simply, the parties cannot rely on it because it is not enforceable."
- "Discovery is complete. That is to say, the discovery process has now finished."
- "The witness was not credible. Essentially, her testimony could not be believed."

## Pass examples
- "The statute of limitations has expired. The plaintiff filed on March 3, sixty days late."
- "The contract is unenforceable. It lacks consideration because neither party promised anything of value."
- "Discovery is complete. Trial is set for June 9."
- "The witness was not credible. She gave three different accounts of the accident."
