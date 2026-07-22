---
id: prefer-concise-phrases
engine: phrase
scope: text
severity: suggestion
intent: style
description: "Flags padded phrases that a shorter word or nothing replaces."
rationale: "Orwell's third rule: if it is possible to cut a word out, cut it out. These stock phrases carry no more meaning than the single word that replaces them."
message: "Cut this padded phrase down to a shorter word."
examples:
  - bad: "Due to the fact that the deadline passed, we filed in order to preserve the claim."
    good: "Because the deadline passed, we filed to preserve the claim."
patterns:
  - pattern: '(?i)\bdue to the fact that\b'
    message: "Replace “due to the fact that”."
    suggestion: "Use “because”."
    fix: "because"
  - pattern: '(?i)\bin order to\b'
    message: "Replace “in order to”."
    suggestion: "Use “to”."
    fix: "to"
  - pattern: '(?i)\bin the event that\b'
    message: "Replace “in the event that”."
    suggestion: "Use “if”."
    fix: "if"
  - pattern: '(?i)\bat this point in time\b'
    message: "Replace “at this point in time”."
    suggestion: "Use “now”."
    fix: "now"
  - pattern: '(?i)\bat the present time\b'
    message: "Replace “at the present time”."
    suggestion: "Use “now”."
    fix: "now"
  - pattern: '(?i)\bfor the purpose of\b'
    message: "Replace “for the purpose of”."
    suggestion: "Use “to” or “for”."
  - pattern: '(?i)\bin spite of the fact that\b'
    message: "Replace “in spite of the fact that”."
    suggestion: "Use “although”."
    fix: "although"
  - pattern: '(?i)\bwith regard to\b'
    message: "Replace “with regard to”."
    suggestion: "Use “about”."
    fix: "about"
  - pattern: '(?i)\bwith respect to\b'
    message: "Replace “with respect to”."
    suggestion: "Use “about”."
    fix: "about"
  - pattern: '(?i)\bin relation to\b'
    message: "Replace “in relation to”."
    suggestion: "Use “about”."
    fix: "about"
  - pattern: '(?i)\bon a regular basis\b'
    message: "Replace “on a regular basis”."
    suggestion: "Use “regularly”."
    fix: "regularly"
  - pattern: '(?i)\bin a timely manner\b'
    message: "Replace “in a timely manner”."
    suggestion: "Use “promptly”."
    fix: "promptly"
  - pattern: '(?i)\bin the near future\b'
    message: "Replace “in the near future”."
    suggestion: "Use “soon”."
    fix: "soon"
  - pattern: '(?i)\bthe majority of\b'
    message: "Replace “the majority of”."
    suggestion: "Use “most” or “most of”."
  - pattern: '(?i)\ba (?:large )?number of\b'
    message: "Replace “a number of”."
    suggestion: "Use “many” or “some”, or give the count."
  - pattern: '(?i)\b(?:has the ability to|have the ability to|is able to|are able to)\b'
    message: "Replace this phrase with “can”."
    suggestion: "Use “can”."
    fix: "can"
  - pattern: '(?i)\bwhen it comes to\b'
    message: "Cut “when it comes to”."
    suggestion: "State the subject directly."
  - pattern: '(?i)\bin today[''’]s world\b'
    message: "Cut “in today’s world”."
    suggestion: "Start with the point."
  - pattern: '(?i)\bin the age of\b'
    message: "Cut “in the age of”."
    suggestion: "Name the present condition directly."
  - pattern: '(?i)\bin the world of\b'
    message: "Cut “in the world of”."
    suggestion: "Name the subject directly."
  - pattern: '(?i)\bthe reality is\b'
    message: "Cut “the reality is”."
    suggestion: "State the reality directly."
  - pattern: '(?i)\bthe truth is\b'
    message: "Cut “the truth is”."
    suggestion: "State the point directly."
  - pattern: '(?i)\bgoing forward\b'
    message: "Cut “going forward”."
    suggestion: "State when or what happens next."
  - pattern: '(?i)\blet[''’]s dive in\b'
    message: "Cut “let’s dive in”."
    suggestion: "Start with the substance."
  - pattern: '(?i)\bat its core\b'
    message: "Cut “at its core”."
    suggestion: "State the central point directly."
  - pattern: '(?i)\bit[''’]s important to note\b'
    message: "Cut “it’s important to note”."
    suggestion: "State the important point directly."
---
