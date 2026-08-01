---
id: imperative-instructions
engine: inferential
scope: prose
severity: suggestion
intent: style
description: Flags instructions written as descriptions or indirect suggestions.
message: Write the instruction in the imperative.
granularity: sentence
---
Flag a procedural instruction when it describes what the reader will want, should, or must do instead of starting with a direct imperative. Do not flag descriptive text, a warning label, or a sentence that states a system requirement rather than instructing the reader.

## Flag examples
- "You will want to restart the service after the update."
- "The client must be restarted after the update."
- "The configuration should be changed before you continue."

## Pass examples
- "Restart the service after the update."
- "Restart the client after the update."
- "Change the configuration before you continue."
