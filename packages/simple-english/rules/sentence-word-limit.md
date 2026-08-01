---
id: sentence-word-limit
engine: statistical
scope: prose
severity: warning
intent: style
description: Flags descriptive sentences longer than 25 words.
rationale: STE Rule 6.3 sets a 25-word descriptive limit. Procedural text has a 20-word limit under Rule 5.1, but the engine cannot classify passages as procedural or descriptive. Quoted code, identifiers, numbers, titles, and text in parentheses count as one word under Rules 8.5-8.7.
message: Shorten this sentence to 25 words or fewer.
metric: sentence-length
params: { max_words: 25 }
examples:
  - bad: "The service stores each request in the audit log and sends a response after it validates the token, checks the account, and applies the configured access policy."
    good: "The service validates the token and stores the request in the audit log."
---
