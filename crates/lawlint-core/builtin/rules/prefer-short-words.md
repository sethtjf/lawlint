---
id: prefer-short-words
engine: phrase
scope: text
severity: suggestion
intent: style
description: "Flags long words that have a short everyday equivalent."
rationale: "Orwell's second rule: never use a long word where a short one will do. Swap the long word only where the plain word carries the same meaning; keep terms of art."
message: "Prefer a shorter, plainer word."
examples:
  - bad: "The parties will utilize the platform to facilitate discovery."
    good: "The parties will use the platform to help discovery."
patterns:
  - pattern: '(?i)\butili[sz]e\b'
    message: "Prefer “use” over “utilize”."
    suggestion: "Use “use”."
    fix: "use"
  - pattern: '(?i)\butili[sz]ation\b'
    message: "Prefer “use” over “utilization”."
    suggestion: "Use “use”."
    fix: "use"
  - pattern: '(?i)\bfacilitate\b'
    message: "Prefer “help” over “facilitate”."
    suggestion: "Use “help”."
    fix: "help"
  - pattern: '(?i)\bcommence\b'
    message: "Prefer “start” over “commence”."
    suggestion: "Use “start” or “begin”."
    fix: "start"
  - pattern: '(?i)\bterminate\b'
    message: "Prefer “end” over “terminate”."
    suggestion: "Use “end”."
    fix: "end"
  - pattern: '(?i)\bendeavou?r\b'
    message: "Prefer “try” over “endeavor”."
    suggestion: "Use “try”."
    fix: "try"
  - pattern: '(?i)\bascertain\b'
    message: "Prefer “find out” over “ascertain”."
    suggestion: "Use “find out”."
    fix: "find out"
  - pattern: '(?i)\bdemonstrate\b'
    message: "Prefer “show” over “demonstrate”."
    suggestion: "Use “show”."
    fix: "show"
  - pattern: '(?i)\bsufficient\b'
    message: "Prefer “enough” over “sufficient”."
    suggestion: "Use “enough”."
    fix: "enough"
  - pattern: '(?i)\bnumerous\b'
    message: "Prefer “many” over “numerous”."
    suggestion: "Use “many”."
    fix: "many"
  - pattern: '(?i)\bapproximately\b'
    message: "Prefer “about” over “approximately”."
    suggestion: "Use “about”."
    fix: "about"
  - pattern: '(?i)\binitiate\b'
    message: "Prefer “start” over “initiate”."
    suggestion: "Use “start”."
    fix: "start"
  - pattern: '(?i)\bpurchase\b'
    message: "Prefer “buy” over “purchase”."
    suggestion: "Use “buy”."
    fix: "buy"
  - pattern: '(?i)\bmethodology\b'
    message: "Prefer “method” over “methodology”."
    suggestion: "Use “method”."
    fix: "method"
  - pattern: '(?i)\bexpedite\b'
    message: "Prefer “speed up” over “expedite”."
    suggestion: "Use “speed up”."
    fix: "speed up"
  - pattern: '(?i)\bremainder\b'
    message: "Prefer “rest” over “remainder”."
    suggestion: "Use “rest”."
    fix: "rest"
  - pattern: '(?i)\bassist\b'
    message: "Prefer “help” over “assist”."
    suggestion: "Use “help”."
    fix: "help"
---
