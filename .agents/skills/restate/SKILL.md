---
name: restate
description: Status of Restate (durable execution) in this repo after the #1939 retirement — indexing now dispatches unconditionally to agentd actor kinds (nrt-index / matter-index / file-index-item / file-ingestion, ADR 0059/0060, epic #1940); the Restate cluster/operator/indexer-restate k8s manifests are deleted; apps/indexer-restate now only hosts the live EvalRun and MatterBrief workflows; apps/sync-restate is the only remaining (dormant, undeployed) Restate consumer. Also covers querying the official Restate docs via the restate-docs MCP server for historical/ADR work. Use when touching indexing orchestration history, the sync-restate engine, or when you need authoritative Restate SDK/server context for ADRs.
---

# Restate in Litvue/agents — RETIRED for indexing

Restate was our durable-execution engine for the **indexing pipeline** (ADR 0009)
and graph construction (ADR 0011). That plane is **retired** (#1939, epic #1940):
indexing orchestration now runs as **agentd actor kinds** inside the agentd
worker pool (ADR 0059/0060) — do NOT add new Restate handlers or manifests.

> Authoritative Restate docs remain available via the **`restate-docs` MCP
> server** (configured in the repo's `.mcp.json`, endpoint
> `https://docs.restate.dev/mcp`) — useful for historical/ADR work and for the
> dormant sync-restate engine. Use `search_restate` for conceptual questions and
> `query_docs_filesystem_restate` to read specific pages.

## Where indexing lives now (the replacement)

| Piece | Path | What it is |
|-------|------|------------|
| Actor kinds | `apps/agentd/src/actor/` | `file-ingestion` (real-pipeline effects adapter), `nrt-index` (ported `NrtIndex`), `matter-index` + `file-index-item` (ported batch `IndexingWorkflow` over the ADR 0060 fan-out primitive). Registered by the worker behind the `indexing` cargo feature + `AGENTD_INDEXING_INGESTION=1` (`apps/agentd/src/roles/worker.rs`). |
| Dispatch | `apps/api-rs` (`indexing/`) | Trigger-only: `index.start` fires a one-way command at the agentd gateway (`POST /v1/sessions/{kind}:{id}/commands`). Indexing dispatch is unconditionally agentd-only; there is no `INDEXING_DISPATCH` env var or Restate fallback. |
| Worker wiring | `k8s/agentd/` | Worker Deployment carries the indexing env (`AGENTD_INDEXING_INGESTION`, `DATABASE_URL`, `AZURE_STORAGE_CONNECTION_STRING` via `agentd-indexing-secrets`, `INDEX_PROFILE`, embedder model-cache emptyDir). |
| Observability | inspector surfaces (ADR 0062) | Actor kind index + inspector event stream + fan-out tree fold; local-dev inspector UI in `apps/web`. Run history lives in the actors' journaled state, read through these surfaces. |
| Decisions | `docs/decisions/0059-*.md`, `0060-*.md`, `0061-*.md`, `0062-*.md` | The agentd actor framework, the fan-out/join primitive + cutover map, workflow versioning, and actor observability. ADR 0009/0011 remain as history. |

## What was deleted / retired (#1939)

- **Deleted manifests:** `k8s/restate` (RestateCluster CR), `k8s/restate-operator`
  (HelmRelease), `k8s/indexer-restate` (RestateDeployment CR + PDB/NetworkPolicy/
  SA/ExternalSecret), and the `infra/modules/aks/main.tf` Flux Kustomizations +
  `enable_restate_indexing` variable + `restate_snapshots` federated identity.
- **Retired-in-place source:** `apps/indexer-restate` keeps the live `EvalRun`
  and `MatterBrief` workflows. The retired indexing handlers
  (`IndexingWorkflow`, `NrtIndex`, `GraphReduce`, and the indexing ledger) were
  deleted; the crate remains until those live workflows migrate elsewhere.
- **NRT `mark_dirty` hand-off:** the `RESTATE_INGRESS_URL` read and `mark_dirty`
  step were removed from `nrt-index`; graph reduce now runs in-process on
  `matter-index` reconciles.

## Sync import migration

External imports now run through the durable agentd `sync-import` actor, with
connector effects served by the stateless `apps/sync` runner. The old
`apps/sync-restate` and `k8s/sync-restate` surfaces were retired in issue #1961.

## Key invariants that carried over (do not violate)

- **No `index_jobs` table, no indexing status-read API.** Status is read from the
  graph (`nodes.indexed_at` / graph presence); run history lives in the actors'
  journaled state (inspector surfaces, ADR 0062).
- **All inference still egresses through `apps/llm-proxy`** (ADR 0007 Decision 1) —
  the in-process embedder loads encoders locally, but no provider keys live in
  the worker.
- **Durable steps stay journaled**: new indexing work goes in workflow-kind
  durable steps (the agentd equivalent of `ctx.run`), never bare side effects.
