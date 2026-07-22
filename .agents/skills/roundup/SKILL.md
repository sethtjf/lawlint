---
name: roundup
description: Round up all outstanding work for the repo — open PRs, open issues, and Devin cloud sessions — into a single issue↔PR↔session graph, then produce one prioritized attack plan that lands in-flight work and launches new work. Use when the user wants to review open PRs, plan attack on open issues, reconcile in-flight Devin sessions, clean up the backlog, or do a "roundup".
---

# Roundup

Reconcile **everything in flight and in the backlog** into one picture, then hand the user a single glanceable attack plan they can sign off on. One graph, two verbs:

- **Land** — finish existing work: merge / close / relabel open PRs, fix missing issue links, clean up sessions.
- **Launch** — start new work from the backlog: dispatch a Devin cloud session, deliver an item locally (the `deliver` skill), or spin up a read-only scoping subagent per actionable issue.

The full per-item plan lives in `todo_write`; a concise **Roundup** goes to chat for sign-off. Do NOT take write actions (merge, close, comment, relabel, dispatch, terminate a session) until the user approves.

## Default execution model: enqueue and return

Roundup is a coordinator, not a CI watcher. After approval, default to asynchronous progress:

- Enqueue approved PRs once; when the merge queue accepts them, report them as **queued** rather than waiting for merge-group CI, merge, or deployment.
- Launch work that is independent of newly queued PRs immediately. Keep only directly dependent launches in **WAITING**.
- Do not run `gh run watch`, sleep loops, or repeated status polling in the main session. One lightweight status snapshot after writes is enough.
- Return control to the user with queued / launched / waiting state while GitHub Actions and cloud sessions continue independently.
- Wait synchronously only when the user explicitly asks to “wait until merged/deployed”, “land fully”, or otherwise requests strict completion.

A pending CI, merge-queue, or deployment run is not a failure and must not monopolize the main session. A completed failed latest default-branch deployment remains a global blocker as described below.

## Tooling

GitHub actions use the `gh` CLI. Devin tools by runtime — use whichever your environment exposes:

| Capability | Devin CLI | `devin` MCP server |
|------------|-----------|--------------------|
| Find / inspect cloud sessions | `devin_session_search` / `devin_session_interact` (action `get`) | same (call `mcp_list_tools` once for schemas) |
| Launch a cloud session | `cloud_handoff` | `devin_session_create` |
| Deliver an item locally | the `deliver` skill (director + `implementer` subagent → PR) | same |
| Read-only scoping agent | `run_subagent` (profile `subagent_explore`) | `run_subagent` |

When prompting for decisions, always use the interactive multiple-choice tool (not free-form chat) and mark one recommended option with `★` plus a why in its description. No such tool → numbered list in chat with the recommendation marked.

## Workflow

### 1. Gather everything (always a full sweep)
However the request is phrased, gather all three entity types — the cross-reference is the whole value.

- **PRs**: `gh pr list --state open --json number,title,headRefName,author,isDraft,createdAt,updatedAt,labels,url,body,mergeable,reviewDecision,statusCheckRollup`
- **Issues**: `gh issue list --state open --json number,title,body,labels,url,comments,assignees,milestone`
- **Sessions**: `devin_session_search` filtered to this repo (owner/repo from `git remote -v`), most-recently-updated first.
- **Deployment health**: `gh issue list --state open --label deploy-blocker` — an open `deploy-blocker` means the latest completed default-branch `Deploy / Pipeline` run failed (see `.github/workflows/AGENTS.md#deployment-health-signal`). PR CI / prebuild / merge-queue green is **not** deployment evidence. If useful, take one snapshot of recent runs to distinguish a newer pending deployment from the last completed result; never wait for it.

An open `deploy-blocker` is the canonical **global blocker**. Surface it first and pause Launch by default, allowing unrelated progress only with explicit user override. If no blocker is open and a newer deployment is pending, report `pending · last completed deployment healthy` rather than blocking. Recovery is proven when a full successful `Deploy / Pipeline` run containing the fix causes the deployment-health observer to close the blocker; observe that on a later status check or Roundup.

### 2. Analyze (fan out for scale, read-only)
≤10 actionable items: analyze inline. More: spawn parallel **read-only** background subagents (`subagent_explore`) over batches of issues (specification quality, files touched, feasibility, blockers) and PRs (are review comments addressed?). Subagents in this phase **never edit** — they share one working tree. For ambiguous PRs, `gh pr view <n> --comments`; for unclear sessions, `devin_session_interact` (action `get`).

### 3. Build the work graph
Draw edges: **PR↔issue** (`Closes/Fixes #N`, branch names, title overlap); **PR↔session** (`devin/*` branches, session URLs/IDs in body/comments); **issue↔session** (session links in comments, search matches); **blocking** (`blocked by #N` / `depends on #N` — across verbs: a launch may be blocked by a PR that must land first).

Flag mismatches: PRs with no session, sessions with no PR, merged work whose issue stayed open, completed PRs missing their issue reference.

### 4. Classify each node
An issue linked to an open PR or live session is **not** its own launch candidate — show it with its linked PR/session so you never dispatch a duplicate.

**PR statuses**: `ready-to-merge` (non-draft, mergeable, required checks passing, no blocking review, review comments addressed — in this repo `reviewDecision` may be empty even for ready PRs; don't require formal approval when the user or context says it's ready; bot `COMMENTED` reviews aren't blockers unless they raise unresolved correctness/security issues) · `needs-review` · `needs-fixes` · `stale` · `superseded` · `close`.

**Issue lanes** (no in-flight PR/session; decide from content, sanity-check labels):

| Lane | When |
|------|------|
| **cloud** | Well-specified, self-contained → Devin cloud session (the remote implementation lane) |
| **local** | Well-specified, but the user wants it done here → the `deliver` skill (director + `implementer` subagent, ending in a PR). The local mirror of the cloud lane |
| **subagent** | Under-specified → read-only scoping subagent (feasibility + implementation brief; never edits) |
| **human** | Needs design, product judgment, or risky/irreversible decisions |
| **skip** | `needs-info`, `wontfix`, duplicate, non-actionable — surface with a one-line reason |

Cloud vs local is the user's call, not an intrinsic property of the issue —
default well-specified items to **cloud** and let the user pull any into **local**
when they want to drive it here.

**Session statuses**: `running`, `sleeping`, `finished`, `errored`, `orphaned` (no linked PR or issue).

### 5. Build per-launch briefs
For each cloud/subagent item: issue summary, acceptance criteria, relevant files, constraints. Cloud briefs use repo-relative paths only (different filesystem; git context is added automatically by `cloud_handoff`/`devin_session_create`). Subagent briefs state the task is read-only scoping.

### 6. Track the full plan
Write every concrete action into `todo_write` (enqueue PR #N, comment+close #M, dispatch session for #Z, terminate stale session, …). Mark cross-verb-blocked launches `waiting on #N`. Distinguish actions this invocation controls from external outcomes: once GitHub accepts a queue submission, complete the **enqueue** todo and represent the eventual merge/deployment as monitored external state rather than leaving the main task in progress. This is the source of truth. If a durable copy is useful, write it to the OS temp dir — never the working tree.

### 7. Surface the Roundup

Decision-first, not entity-first. Rendered markdown, **no code fences**. Default surface = headline + action-grouped thread list only; entity tables and dependency graph are opt-in drill-downs. The user should know exactly what to do next, in order, without joining rows across tables.

**Layer 1 — headline** (≤6 lines, `Recommended:` first, omit empty lines):

**Recommended:** Enqueue #123 #131, launch 2 cloud sessions, terminate 1 orphan
**Deployment:** healthy at abc1234 · pending at def5678 · or · **BLOCKED:** 4 consecutive main deploy failures since abc1234
**Blockers:** #202 waiting on #126 (failing clippy)
**PRs:** 3 ready-to-merge · 1 needs-fixes · **Issues:** 8 open — 3 in-flight · 2 cloud-ready · **Sessions:** 4 running · 1 orphaned

**Layer 2 — action-grouped thread list**: bold group headers + one single-line bullet per thread (driving id, short title, concrete action, linked entities + blockers inline). Bullets, never leading-space indentation. Omit empty groups. Groups in order:

**DEPLOYMENT BLOCKER**
- **main** Deploy / Pipeline — `land fix → observe a later full deploy success` · 4 consecutive failures since abc1234 · all other Land/Launch paused

**LAND — ready now**
- **#123** Fix auth timeout — `enqueue` · closes #45 · session devin-abc (sleeping)

**DECIDE — needs you**
- **#126** Embedder refactor — `fix clippy → enqueue` · closes #78

**LAUNCH — new work**
- **#203** Flaky webhook retries — `dispatch cloud session`
- **#118** Tidy config loader — `deliver locally` (well-specified; user driving it here)

**WAITING**
- **#202** Add ColBERT scoring — `cloud after #126 merges and deploys` · directly blocked by #126

**MONITOR**
- session devin-def `running 10m` → #200 Add retry logic
- PR #123 `queued` · CI/deployment continues asynchronously

**CLEAN UP**
- session devin-xyz `orphaned 3d` → terminate

**Layer 3 — drill-downs (opt-in via the Extras question, never by default)**: full per-entity tables (PRs: # / title / status / linked issues / session / action · Issues: # / title / in-flight? / lane / blocked-by / action · Sessions: id / status / since / linked PR / issue / action) and a cluster-based dependency graph for complex blocking chains.

### 8. Prompt with a scoped question chain
One question per section with actionable items, each via the multiple-choice tool with cherry-pickable options and one `★` default chosen from the actual plan:

0. **Deployment** (if blocked by a completed failure): `★ Pause Land/Launch; fix deployment first` / `Proceed with selected unrelated work` / `Let me inspect failures`. Dequeuing already-approved queued PRs still requires explicit approval.
1. **PRs** (if landable): `★ Enqueue all N ready PRs (#…)` / `Enqueue selected — let me pick` / `Skip PRs`
2. **Launches** (if candidates): `★ Launch all K independent items (X cloud + Y subagent)` / `Launch cloud only` / `Deliver selected locally` / `Let me pick` / `Skip`. Show directly dependent items as waiting, not as launchable. When the user picks items for **local** delivery, route each through the `deliver` skill instead of `cloud_handoff`.
3. **Cleanup** (if orphans/mismatches/closeables): `★ Do all cleanup (…)` / `Skip cleanup`
4. **Extras** (always): `Show full entity tables` / `Show the dependency graph` / `★ Proceed` / `Let me adjust first`

Batch all applicable questions into a single prompt if the environment supports it; otherwise ask sequentially. Do NOT execute any write action until the chain completes.

### 9. On approval: enqueue Land → launch independent work → return

**Deployment-health gate:** if an open `deploy-blocker` issue exists, the latest completed default-branch deployment failed — **pause all Launches by default**. Landing the deployment fix and cleanup is allowed; unrelated Launch requires explicit override. Do not wait for the observer to close the blocker in this invocation.

1. **Immediate Land actions** — perform approved comments, closes, issue-link fixes, relabeling, and session cleanup.
2. **Enqueue PRs** — submit approved ready PRs to the required merge queue. Once accepted, mark the enqueue actions complete and report queue position/state. Do not wait for merge-group CI, merge, or deployment.
3. **Partition Launch**:
   - If no `deploy-blocker` is open, launch items independent of newly queued PRs now: cloud → `cloud_handoff` / `devin_session_create`; subagent → background `run_subagent` with `subagent_explore`.
   - Keep items directly dependent on queued PRs in **WAITING**. If the dependency changes deployed behavior, require a later observed full deployment success containing the merge before releasing the launch.
   - If a `deploy-blocker` is open, keep all Launch items waiting unless the user explicitly approved unrelated progress.
   - Dispatch independent cloud/subagent work before starting approved local deliveries. Run local `deliver` items one at a time because they occupy the main session.
4. **Record linkage** — for every dispatched issue, `gh issue comment <n>` with the session/PR URL (or subagent note) and `gh issue edit <n> --add-label "in-progress"` (create the label if missing). For local items `deliver` handles linkage itself.
5. **Take one snapshot** — optionally read queue/PR/deployment status once after writes. Never use `gh run watch`, sleep loops, repeated polling, or wait on pending external work in the default mode.
6. **Return promptly** — summarize `queued / launched / waiting / cleanup`. A queue submission is not “merged”; label it accurately. If there are no Launch candidates, there is never a reason to wait for deployment.

On a later status check or Roundup:

1. Inspect queued PRs and the latest completed full deployment once.
2. Drop work whose PR merged, note auto-closed issues, and release directly dependent launches only when their required merge/deployment evidence exists.
3. If the latest completed full deployment failed, add the run/root cause to the graph and apply the global deployment blocker policy.
4. If CI or deployment is still pending after a last completed success, report it as pending and return without polling.

**Strict completion override:** only when the user explicitly requests waiting until merge/deployment may the main session block on external checks. Keep that exception scoped to the requested PRs/runs; it is not the Roundup default.

Tick off immediate actions as each completes. Represent queued and waiting external outcomes accurately rather than keeping the coordinator occupied.

### 10. Prompt for the follow-up
Print a short summary (queued / launched / waiting), then prompt again via the multiple-choice tool, tailored to what remains: `★ Continue independent work` / `Check queued PRs once` / `Check dispatched sessions once` / `Re-run roundup` / `Done for now`. A status check is one snapshot, never a polling loop.

Destructive/irreversible actions (closing PRs/issues, terminating sessions) always require explicit approval per the safety rules.
