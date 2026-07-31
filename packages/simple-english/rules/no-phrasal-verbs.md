---
id: no-phrasal-verbs
engine: phrase
scope: prose
severity: suggestion
intent: style
description: Flags common phrasal verbs with clearer single-word alternatives.
rationale: Single-word verbs are easier to translate and less likely to have several meanings. Choose the replacement that fits the context.
message: Prefer a clear single-word verb where the meaning is unchanged.
allow_context:
  pattern: '`'
  window: 1
examples:
  - bad: "Set up the service, then shut down the old process."
    good: "Configure the service, then stop the old process."
patterns:
  - pattern: '(?i)\bset up\b'
    message: "Replace “set up” with “install” or “configure”, as appropriate."
    suggestion: "Choose the precise single-word verb."
  - pattern: '(?i)\bshut down\b'
    message: "Replace “shut down” with “stop”."
    suggestion: "Use “stop” when that is the intended meaning."
  - pattern: '(?i)\bgo down\b'
    message: "Replace “go down” with “decrease” or “fail”, as appropriate."
    suggestion: "Choose the precise verb."
---
  - pattern: '(?i)\bgo up\b'
    message: "Replace “go up” with “increase” or “start”, as appropriate."
    suggestion: "Choose the precise verb."
  - pattern: '(?i)\bcarry out\b'
    message: "Replace “carry out” with “do” or a more precise verb."
    suggestion: "Choose the precise verb."
  - pattern: '(?i)\bfind out\b'
    message: "Replace “find out” with “determine” or “learn”, as appropriate."
    suggestion: "Choose the precise verb."
  - pattern: '(?i)\bbring up\b'
    message: "Replace “bring up” with a precise verb."
    suggestion: "State whether you mean display, mention, or raise."
  - pattern: '(?i)\bpoint out\b'
    message: "Replace “point out” with “identify” or “show”, as appropriate."
    suggestion: "Choose the precise verb."
  - pattern: '(?i)\bput in place\b'
    message: "Replace “put in place” with “implement” or “install”, as appropriate."
    suggestion: "Choose the precise verb."
