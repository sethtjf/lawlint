---
id: technical-noun-as-verb
engine: inferential
scope: prose
severity: suggestion
intent: style
description: Flags technical nouns used as verbs when a direct construction is clearer.
message: Use the technical noun with a direct verb.
granularity: sentence
---
Flag a sentence that uses a technical noun as a verb instead of using a direct construction. For example, “webhook the event” should become “send the event to the webhook,” and “do a deploy” should become “deploy.” Do not flag established domain verbs such as deploy, compile, merge, or commit.

## Flag examples
- "Webhook the event after you create the record."
- "Do a deploy after you test the release."
- "We need to API the new endpoint."

## Pass examples
- "Send the event to the webhook after you create the record."
- "Deploy after you test the release."
- "Call the new endpoint with the client."
