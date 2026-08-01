---
id: safety-warning-order
engine: inferential
scope: prose
severity: warning
intent: style
description: Flags safety warnings that bury the command or condition after the risk.
message: Put the risk word first, then the command or condition, then the consequence.
granularity: sentence
---
Flag a safety warning when it starts with background or a possible consequence and gives the command later. A compliant warning starts with WARNING or CAUTION, gives a direct command or condition, and then states the risk or result. Do not flag an ordinary descriptive risk statement.

## Flag examples
- "Data loss can occur if you use the force flag in production. Do not use it."
- "The database can be damaged, so do not run the cleanup command."
- "If the flag is enabled, rows can be deleted. Use the flag only in a test environment."

## Pass examples
- "CAUTION: Do not use the force flag in production. It deletes rows."
- "WARNING: If the database is live, do not run the cleanup command."
- "CAUTION: Use the flag only in a test environment. It can delete rows."
