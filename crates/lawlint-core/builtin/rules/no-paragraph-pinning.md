---
id: no-paragraph-pinning
engine: inferential
scope: text
severity: warning
# Judged train smoke (20 AI / 20 human): flagged 0 AI and 0 human rows;
# unmeasured support, so this remains a style lint.
intent: style
description: "Flags paragraphs that open and close by repeating the same theme."
rationale: "Returning to the opening phrase as a closing beat pins the paragraph instead of moving the analysis forward."
message: "End the paragraph with its consequence or next point, not its opening theme."
examples:
  - bad: "The record raises a notice problem. The parties exchanged several letters, but none stated the contractual deadline. The notice problem therefore remains the notice problem."
    good: "The record raises a notice problem. The parties exchanged several letters, but none stated the contractual deadline. The court should therefore construe the notice provision against the drafter."
granularity: paragraph
---
Flag a paragraph when its opening theme or distinctive phrase returns in
the closing sentence merely to pin the paragraph's point. The return may
repeat the same words or restate the same idea without adding a consequence,
qualification, or new direction. Do not flag a paragraph that ends with a
new implication, result, or recommendation. Do not flag repetition required
to identify a defined legal term, party, claim, or statutory element.

## Flag examples
- "The record presents a serious timing problem. The parties dispute when the notice arrived, and the file contains no delivery receipt. The timing problem remains the timing problem."
- "The statute gives the agency broad discretion. The agency considered the relevant reports and heard from both sides. That broad discretion is the central point."
- "The contract contains an integration clause. The parties exchanged drafts and signed the final version. The integration clause is still the integration clause."
- "The witness's account is uncertain. She changed the date twice and could not identify the source of her estimate. The uncertainty is the real issue."

## Pass examples
- "The record presents a serious timing problem. The parties dispute when the notice arrived, and the file contains no delivery receipt. The court should remand for a finding on delivery."
- "The statute gives the agency broad discretion. The agency considered the relevant reports and heard from both sides. The decision therefore falls within the statute's zone of reasonableness."
- "The contract contains an integration clause. The parties exchanged drafts and signed the final version. The clause bars reliance on the earlier representations."
- "The witness's account is uncertain. She changed the date twice and could not identify the source of her estimate. The testimony cannot support summary judgment."
