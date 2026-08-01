---
id: consistent-terminology
engine: inferential
scope: prose
severity: warning
intent: style
description: Flags rotation between different words for the same concept.
message: Choose one term for this concept and use it throughout the document.
granularity: document
---
Flag a document that rotates between synonyms for one technical concept. In particular, flag rotation among check, verify, confirm, and validate when they mean the same action, or among config, configuration, settings, and options when they mean the same object. Do not flag terms that clearly have different meanings or terms used in quoted code, identifiers, commands, or product names.

## Flag examples
- "Verify the token is valid. Confirm the account before you continue."
- "Update the config file. The settings control the timeout."
- "Confirm the value, then validate the request."

## Pass examples
- "Make sure that the token is valid. Make sure that the account is active."
- "Update the configuration file. The configuration controls the timeout."
- "Validate the value, then validate the request."
