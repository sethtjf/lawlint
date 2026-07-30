---
id: no-summary-beat
engine: inferential
scope: text
severity: warning
# Judged train smoke (20 AI / 20 human): flagged 0 AI and 0 human rows;
# unmeasured support, so this remains a style lint.
intent: style
description: "Flags closing sentences that recap a paragraph without adding information."
rationale: "A summary beat announces the paragraph's point after the analysis is complete instead of carrying the analysis forward."
message: "Remove the closing summary beat or replace it with a consequence."
examples:
  - bad: "The notice omitted the required date, so the agency could not verify when the appeal period began. The upshot is simple."
    good: "The notice omitted the required date, so the agency could not verify when the appeal period began. The appeal was therefore timely."
granularity: paragraph
---
Flag a paragraph's closing sentence when it merely recaps the paragraph
with a phrase such as "the upshot is simple", "that is the point", "the
lesson is clear", or "this is the tension", without adding a fact, result,
qualification, or next step. Do not flag a conclusion that adds a concrete
holding, consequence, number, exception, or recommendation. Do not flag
padded elaboration merely because a sentence follows another one: padded
elaboration restates the previous sentence, while a summary beat recaps
the paragraph as a whole.

## Flag examples
- "The notice omitted the required date, and the agency could not verify when the appeal period began. The upshot is simple."
- "The parties disputed the clause's meaning, and the court adopted the narrower reading. That is the point."
- "The record contains conflicting accounts and no neutral witness. That is the tension."
- "The rule has exceptions, but none applies here. The lesson is clear."

## Pass examples
- "The notice omitted the required date, and the agency could not verify when the appeal period began. The appeal was therefore timely."
- "The parties disputed the clause's meaning, and the court adopted the narrower reading. That reading excludes consequential damages."
- "The record contains conflicting accounts and no neutral witness. The court should resolve the dispute at trial."
- "The rule has exceptions, but none applies here. The motion is denied."
