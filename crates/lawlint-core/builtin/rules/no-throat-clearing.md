---
id: no-throat-clearing
engine: leading
scope: text
severity: error
description: "Flags throat-clearing openers"
rationale: "Start with the substance. Openers that add no information should be cut."
message: "Cut the throat-clearing and lead with the point."
examples:
  - bad: "Let me think about this. The claim fails."
    good: "The claim fails."
patterns:
  - pattern: 'let me think(?: about this)?'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Cut the throat-clearing and lead with the point."
  - pattern: 'here[''’]s my take'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Cut the throat-clearing and lead with the point."
  - pattern: 'here[''’]s what i think'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Cut the throat-clearing and lead with the point."
  - pattern: 'i think it[''’]s worth'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Cut the throat-clearing and lead with the point."
  - pattern: 'before (?:i|we) (?:begin|start|dive in)'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Cut the throat-clearing and lead with the point."
  - pattern: '(?i)\bas we all know\b'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "State the shared fact directly."
  - pattern: '(?i)\bit goes without saying\b'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Say the point or omit it."
  - pattern: '(?i)\bbefore diving in\b'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Begin with the substance."
  - pattern: '(?i)\bto set the stage\b'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Begin with the relevant fact."
  - pattern: '(?i)\bfirst,\s+some context\b'
    message: "Cut the throat-clearing and lead with the point."
    suggestion: "Give the relevant context without a preface."
---
