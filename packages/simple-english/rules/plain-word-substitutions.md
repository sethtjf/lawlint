---
id: plain-word-substitutions
engine: phrase
scope: prose
severity: suggestion
intent: style
description: Flags common inflated or vague words and phrases.
rationale: Plain words improve translation and make technical facts easier to understand. Keep a technical term when it carries a precise domain meaning.
message: Prefer a plain word or state the measurable fact.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "The robust service seamlessly enables you to leverage the functionality out of the box."
    good: "The service lets you use the feature by default."
patterns:
  - pattern: '(?i)\bleverage\b'
    message: "Use “use”."
    suggestion: "Replace “leverage” with “use”."
    fix: "use"
  - pattern: '(?i)\bprior to\b'
    message: "Use “before”."
    suggestion: "Replace “prior to” with “before”."
    fix: "before"
  - pattern: '(?i)\bensure\b'
    message: "Use “make sure that” when that is the intended meaning."
    suggestion: "Write “make sure that” or state the guarantee precisely."
  - pattern: '(?i)\bit is worth noting that\b'
    message: "Delete “it is worth noting that” and state the fact."
    suggestion: "State the fact directly."
  - pattern: '(?i)\b(?:simply|just|easily|seamlessly|effortlessly)\b'
    message: "Delete this filler or state the measurable property."
    suggestion: "Remove the filler."
  - pattern: '(?i)\b(?:robust|powerful|comprehensive|performant)\b'
    message: "State the measurable property instead of using this vague adjective."
    suggestion: "Give the relevant limit, result, or behavior."
  - pattern: '(?i)\bfunctionality\b'
    message: "Use “function” or “feature”."
    suggestion: "Choose the precise plain noun."
  - pattern: '(?i)\b(?:enables you to|allows you to)\b'
    message: "Use “you can”."
    suggestion: "Rewrite this as “you can”."
    fix: "you can"
  - pattern: '(?i)\b(?:is designed to|aims to)\b'
    message: "State what the system does."
    suggestion: "Delete this phrase and state the behavior directly."
  - pattern: '(?i)\b(?:dive into|delve into)\b'
    message: "Use “read”, “examine”, or another precise verb."
    suggestion: "Choose the verb that describes the action."
  - pattern: '(?i)\b(?:as needed|as necessary)\b'
    message: "State the condition that requires the action."
    suggestion: "Replace this phrase with the condition."
  - pattern: '(?i)\band/or\b'
    message: "Choose the alternatives or write “X, or Y, or both”."
    suggestion: "State the exact alternatives."
  - pattern: '(?i)\bgracefully handles\b'
    message: "State the actual behavior."
    suggestion: "Give the retry, error, or stop behavior."
  - pattern: '(?i)\bout of the box\b'
    message: "Use “by default”."
    suggestion: "Replace “out of the box” with “by default”."
    fix: "by default"
  - pattern: '(?i)\bunder the hood\b'
    message: "Use “internally” or state the implementation."
    suggestion: "State the implementation directly."
  - pattern: '(?i)\bstreamline\b'
    message: "Use “make simpler” or “make faster”, as appropriate."
    suggestion: "State the measurable improvement."
  - pattern: '(?i)\b(?:plethora|myriad)\b'
    message: "Use “many” or give the number."
    suggestion: "State the count or use “many”."
  - pattern: '(?i)\b(?:addresses the issue|tackles)\b'
    message: "State what the system corrects or removes."
    suggestion: "Name the fault and the action."
---
