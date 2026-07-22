---
name: grill
description: Interview the user relentlessly about a plan or design until reaching shared understanding, resolving each branch of the decision tree, and updating documentation (CONTEXT.md, ADRs) inline as decisions crystallise. Use when user wants to stress-test a plan, be interviewed about a design, get grilled, or says "grill me" / "interview me".
---

# Grill

Interview me relentlessly about every aspect of this plan until we reach a shared understanding. Walk down each branch of the design tree, resolving dependencies between decisions one-by-one. For each question, provide your recommended answer.

Ask the questions one at a time, waiting for feedback on each question before continuing.

If a question can be answered by exploring the codebase, explore the codebase instead.

## Domain awareness

During codebase exploration, also read the existing domain docs: root `CONTEXT.md` (glossary) and `docs/adr/` or `docs/decisions/` (decision records). Create them lazily — only when you have something to write.

### During the session

- **Challenge against the glossary.** When the user uses a term that conflicts with `CONTEXT.md`, call it out immediately: "Your glossary defines 'cancellation' as X, but you seem to mean Y — which is it?"
- **Sharpen fuzzy language.** When a term is vague or overloaded, propose a precise canonical term.
- **Discuss concrete scenarios.** Stress-test domain relationships with specific edge-case scenarios that force precision about concept boundaries.
- **Cross-reference with code.** When the user states how something works, check whether the code agrees; surface contradictions.

### Update docs inline

- **CONTEXT.md**: when a term is resolved, update it right there — don't batch. Format in [CONTEXT-FORMAT.md](./CONTEXT-FORMAT.md). `CONTEXT.md` is a glossary only — no implementation details, no spec content.
- **ADRs**: offer one only when the decision is (1) hard to reverse, (2) surprising without context, AND (3) the result of a real trade-off. If any is missing, skip it. Format in [ADR-FORMAT.md](./ADR-FORMAT.md).
