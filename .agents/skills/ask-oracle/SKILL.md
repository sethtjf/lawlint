---
name: ask-oracle
description: Escalate a hard question to a stronger supervisor LLM (default Anthropic Mythos-class fable-5) instead of interrupting the user, by spawning a subagent pinned to that model. Use when stuck after 2+ failed attempts, facing an ambiguous design decision, a risky/irreversible change, or a debugging dead end that a more capable model could resolve.
---

# Ask the Oracle

Like asking the user a question — but the answer comes from a bigger model. The
coding agent keeps running on a cheap/fast model and bubbles individual hard
questions up to a supervisor ("oracle") model, so the expensive model is only
paid for at decision points, not for the whole session.

The oracle is reached by **spawning a subagent native to your own runtime,
pinned to the oracle model** (default: `fable-5`, Anthropic Mythos class). No
provider API key is required — the subagent runs through whatever inference
path your session already uses.

## When to escalate

Escalate to the oracle when ANY of these hold:

- You have attempted a fix/approach 2+ times and are still failing.
- You face a design fork where the options have materially different costs and
  the codebase/docs don't resolve it.
- You are about to do something risky or irreversible and want a second opinion.
- You suspect your own reasoning is wrong (contradictory evidence, confusion).

Do NOT escalate for things you can resolve by reading the codebase, docs, or
running a quick experiment — exhaust those first, exactly as you would before
asking a human. Still ask the *user* (not the oracle) for anything involving
secrets, permissions, product intent, or preferences only they can know.

## How to invoke (by runtime)

Pick the dispatch path for the agent you are running as. In all cases the
subagent's prompt is the full self-contained question (see next section), and
you read its final reply as the oracle's answer.

| Runtime | Dispatch |
|---------|----------|
| Claude Code | Launch the `oracle` subagent (defined in `.claude/agents/oracle.md`, pinned to `fable-5`) via the Task tool. It is read-only by design — it advises, you act. |
| Devin | Spawn a child session via the Devin MCP `create_session` tool, instructing it to act as a one-shot oracle: answer the question and terminate without making changes. |
| Codex / other | Use the runtime's native subagent/delegate mechanism with a model override to `fable-5`. If the runtime cannot pin a subagent model, fall back to the script below. |
| Fallback (direct API) | Only if no subagent mechanism exists AND `ANTHROPIC_API_KEY` is set: `scripts/ask-oracle.sh "question" [< context]`. Supports `ORACLE_MODEL` / `ORACLE_BASE_URL` overrides (e.g. route via llm-proxy). |

If neither a subagent mechanism nor an API key is available, fall back to
asking the user.

## How to ask a good question

The oracle starts with a fresh context — it knows nothing about your task,
your conversation, or what you've already tried. You are responsible for
briefing it. Every question must be a self-contained brief with these parts:

1. **Goal** — what you are ultimately trying to achieve (1-2 sentences),
   including the user's original ask if relevant.
2. **State** — what you've tried, in order, and what happened each time.
   Include exact error output, not paraphrases.
3. **Question** — one specific, decision-shaped question. Prefer "Should I do
   A or B given X?" over "What's wrong?".
4. **Constraints** — anything that rules options out (perf, compat, deadlines,
   repo conventions, explicit user instructions).

### Context-briefing checklist

Before dispatching, include everything the oracle needs to investigate
independently:

- [ ] **File paths** — every file relevant to the question (`path/to/file.rs:42`
      style), so an oracle with repo access can read them itself.
- [ ] **Concepts & domain terms** — define project-specific names the oracle
      won't know (e.g. internal service names, domain vocabulary), or point to
      where they're documented (`AGENTS.md`, `CONTEXT.md`, ADRs).
- [ ] **Key code/error excerpts inline** — don't rely solely on file pointers;
      quote the few lines that matter in case the oracle has no repo access.
- [ ] **Reproduction command** — the exact command that demonstrates the
      problem and its current output.
- [ ] **Branch/commit** — if the relevant state isn't on the default branch.

Keep the brief focused (~50KB max); trim logs to the relevant failure, not the
whole scrollback. A good test: could a new engineer with repo access but zero
conversation history act on your brief without asking a follow-up?

## Using the answer

- Treat the oracle as a strong advisor, not an authority: verify its claims
  against the codebase before acting, same as a human reviewer's suggestion.
- If the answer conflicts with explicit user instructions or repo rules, the
  user/rules win — escalate to the user instead.
- Record the question + answer in your working notes (or the PR description if
  it shaped a decision) so the reasoning is auditable.
- Limit yourself to a few escalations per task. If you're bouncing repeated
  questions off the oracle, the task is under-specified — go back to the user.
