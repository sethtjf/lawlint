---
name: deliver
description: Execute one well-scoped item to completion locally in the main session — plan it, delegate implementation to the `implementer` subagent, verify, commit on a branch, and open a PR. The local counterpart to a cloud handoff. Use when the user wants to do a triaged item locally (from roundup's LOCAL lane or directly), "deliver #N", "implement this here", or "land this locally".
argument-hint: "Issue #, PR to continue, or a scoped task description"
---

# Deliver

Local delivery loop: **you (the director) plan and integrate; the `implementer`
subagent writes the code.** This is the local mirror of `cloud_handoff` — same
triaged item, executed here instead of in the cloud, ending in a PR.

You (the main session) should be running a strong planning model. Do NOT write
the implementation yourself — decompose the work and delegate each chunk to the
`implementer` subagent, then review and integrate.

## Input

One well-scoped item:
- a GitHub issue number (`gh issue view <n>` for context + acceptance criteria),
- an existing PR/branch to continue, or
- a free-form task description.

If the item is under-specified or ambiguous, STOP and scope it first — use a
read-only `subagent_explore` for a feasibility/implementation brief, or ask the
user. Do not delegate implementation of a fuzzy task.

## Workflow

### 1. Plan
Read the issue and the relevant code (directly or via `subagent_explore`).
Produce a short implementation plan and write concrete steps to `todo_write`.
Break the work into **independent, self-contained chunks** wherever possible.

### 2. Set up the branch
Create a feature branch off the up-to-date main branch before any edits (never
work on `main`). Match existing branch conventions.

### 3. Delegate implementation
For each chunk, spawn the **`implementer`** subagent (pinned to `sonnet`) with a
self-contained prompt: the task, acceptance criteria, relevant repo-relative
paths, and constraints. It cannot see this conversation.

- **Parallel background** implementers for genuinely independent chunks — but
  they share one working tree, so never let two touch the same files at once.
- **Foreground** for anything requiring live tool approval, or when chunks are
  sequential/dependent.
- Background subagents can't prompt for permissions — pre-approve the relevant
  `Exec(...)` scopes first, or run in the foreground. Resume a permission-denied
  background run in the foreground to grant access.

Review each implementer's report before integrating. Re-delegate follow-ups as
needed; you own correctness.

### 4. Verify
Run the narrowest relevant checks for what changed (lint, typecheck, build,
targeted tests) per `AGENTS.md` / `just` recipes. Delegate this to the
**`test-runner`** subagent — fan out independent suites (e.g. TS unit + Rust
crate + typecheck) as parallel background runs, or run one foreground. It reports
pass/fail with failure details but never edits. Iterate — re-delegate fixes to
the `implementer` — until green or a genuine blocker surfaces.

### 5. Commit
Commit on the feature branch following `AGENTS.md` **Commit & PR Conventions**
(Conventional Commits: `type(scope): description`, lowercase, imperative). Honor
the repo's commit-message + attribution rules. If pre-commit hooks modify files,
stage them and retry.

### 6. Open the PR
Push the branch and open a PR with `gh pr create`. Conventional-Commit title,
`## Summary` + `#### Test plan` body, and `Closes #N` when delivering an issue.
Then link back on the issue: `gh issue comment <n>` with the PR URL and
`gh issue edit <n> --add-label "in-progress"` (create the label if missing) —
mirroring how roundup records cloud-session linkage for dedup.

Do NOT push or open the PR until verification passes and the user has approved,
per the safety rules. Never force-push, rewrite history, or bypass hooks.

### 7. Report
Summarize: branch, PR URL, files touched, verification result, and any
follow-ups. Tick off the `todo_write` plan as you go.

## Relationship to other primitives

- **roundup** triages and routes an item into this LOCAL lane.
- **cloud_handoff** is the remote alternative for the same kind of item.
- **handoff** compacts context if you need to pass this work to another session
  mid-flight.
