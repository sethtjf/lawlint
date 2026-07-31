---
id: one-instruction-per-sentence
engine: inferential
scope: prose
severity: warning
intent: style
description: Flags procedural sentences that contain more than one instruction.
message: Write one instruction per sentence.
granularity: sentence
---
Flag a procedural sentence when it gives two or more separate commands to the reader. Do not flag two actions that happen at the same time, a command followed by a result, or a descriptive sentence.

## Flag examples
- "Open the file and edit the version."
- "Stop the service, then remove the lock file."
- "Copy the key and restart the client."

## Pass examples
- "Open the file. Edit the version."
- "Stop the service. Then remove the lock file."
- "The client reads the key and sends the request."
