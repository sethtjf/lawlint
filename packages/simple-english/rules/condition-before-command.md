---
id: condition-before-command
engine: inferential
scope: prose
severity: warning
intent: style
description: Flags instructions that put a required condition after the command.
message: Put the condition before the command.
granularity: sentence
---
Flag a procedural instruction when it gives a command before a condition that must be true for the command to be safe or valid. Prefer “If the build fails, read the log.” Do not flag a condition that describes a result after the command, or a descriptive sentence.

## Flag examples
- "Read the log if the build fails."
- "Run the migration when the backup is complete."
- "Increase the timeout if the network is slow."

## Pass examples
- "If the build fails, read the log."
- "When the backup is complete, run the migration."
- "If the network is slow, increase the timeout."
