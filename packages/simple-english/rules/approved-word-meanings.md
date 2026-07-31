---
id: approved-word-meanings
engine: phrase
scope: prose
severity: suggestion
intent: style
description: Flags words used with meanings or parts of speech that can confuse readers.
rationale: Use one clear meaning for each common word. These checks cover frequent technical-writing substitutions, not the complete STE dictionary.
message: Use the plain word with its approved meaning.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "Check that the value is above 10 and follow the instructions."
    good: "Make sure that the value is more than 10 and obey the instructions."
patterns:
  - pattern: '(?i)\bcheck\s+(?:that|if|whether)\b'
    message: "Use “make sure that”."
    suggestion: "Replace “check that”, “check if”, or “check whether” with “make sure that”."
  - pattern: '(?i)\babove\s+\d+(?:\.\d+)?\b'
    message: "Use “more than” for a numerical limit."
    suggestion: "Replace “above” with “more than”."
  - pattern: '(?i)\bbelow\s+\d+(?:\.\d+)?\b'
    message: "Use “less than” for a numerical limit."
    suggestion: "Replace “below” with “less than”."
  - pattern: '(?i)\bfollow\s+the\s+instructions\b'
    message: "Use “obey the instructions”."
    suggestion: "Replace “follow” with “obey”."
    fix: "obey the instructions"
  - pattern: '(?i)\bwith the help of\b'
    message: "Use “with the aid of”."
    suggestion: "Replace “with the help of” with “with the aid of”."
    fix: "with the aid of"
  - pattern: '(?i)\btest\s+the\s+(?:[a-z][\w-]*)(?:\s+[a-z][\w-]*)?\b'
    message: "Use “do a test of” when “test” is a noun."
    suggestion: "Rewrite this as “do a test of …” if that is the intended meaning."
  - pattern: '(?i)\bfall(?:s|ing)?\s+(?:by|to)\b'
    message: "Use “decrease” for a numerical reduction."
    suggestion: "Replace “fall” with “decrease” when you mean a reduction."
---
