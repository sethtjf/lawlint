---
paths:
  - apps/api-rs/**
---

# API Architecture

## Per-Plane Architecture

The platform is split into independent **planes**, each its own deployable. There
is no longer a "single public surface" that everything must route through:
Cloudflare DNS + proxy front every public domain and each public route is
protected by the WorkOS auth boundary, so public exposure is decoupled from
`api-rs`. See [ADR 0004](../../docs/decisions/0004-per-plane-public-boundaries.md).

The canonical taxonomy — the four planes (Data = `apps/api-rs`, Content/Indexing
= `apps/indexer`, AI Inference = `apps/llm-proxy`, Agent Session = `apps/agentd`),
the **non-planes** (`apps/web` + `apps/marketing` are the presentation/client
layer and host the browser-telemetry edge; Postgres/Blob/Redis are the storage
layer the data plane fronts), and the cross-cutting boundaries (telemetry, auth)
that are applied per-plane rather than being planes — is defined in
[ADR 0007](../../docs/decisions/0007-multi-service-plane-architecture.md). The
table below is the operational quick-reference for the publicly-reachable planes.

| Plane | App | Port | Purpose | Public auth |
|-------|-----|------|---------|-------------|
| Dataplane (REST API) | `apps/api-rs/` | :8788 (prod: api.litvue.com) | Graph-native CRUD, Postgres, WorkOS auth, billing | WorkOS JWT |
| Agent Session | `apps/agentd/` | :8789 local (prod: agents.litvue.com, K8s) | Session runtime (gateway + stateless harness pool over the FDB session log; ADR 0022/0034/0037) | WorkOS bearer on connect/commands; per-session `obo_agent` token for data-plane work |
| LLM | `apps/llm-proxy/` | :8791 | OpenAI-compatible `/llm/v1/*` proxy; holds LLM provider keys | WorkOS (AuthKit + session-agent JWT) |
| Telemetry | `apps/web/worker/` | same-origin (`app(-staging).litvue.com/_telemetry`) | Browser telemetry forwarder `POST /_telemetry/v1/{traces,logs}` embedded in the web SPA's Cloudflare Worker | same-origin (anonymous accepted; contextual WorkOS enrichment, PII-scrubbed) |

plus Kubernetes-backed Content/Indexing-plane workers (`apps/indexer`) that are
not publicly reachable. The `/llm` plane was extracted out of `api-rs`
(Litvue/agents#1059) so the product API stops carrying LLM provider keys.
Browser telemetry no longer transits `api-rs` either: it is forwarded
same-origin by the `apps/web` Cloudflare Worker to the otel-collector
gateway, retiring the standalone telemetry-gateway service.

**All AI inference egresses through `apps/llm-proxy`** (ADR 0007, Decision 1):
every plane — including the `Luis` harness — calls the OpenAI-compatible
`/llm/v1/*` endpoint, and no provider keys live outside `llm-proxy`. The
harness already does this via `llmProxyStreamFn` (`apps/luis/src/luis.ts`);
do not add a provider-direct path or mount provider keys in any other plane.

**Indexing orchestration lives on agentd actors, not `api-rs`** (ADR 0059/0060,
epic #1940; ADR 0009 established the trigger-only posture, superseding the
ADR 0006/0007 in-process `IndexerController` + `index_jobs` queue model).
`api-rs`'s indexing surface is **trigger-only**: the `index.start` action on the
unified `POST /nodes/{id}/actions` dispatcher fires a one-way command at the
agentd actor surface (`nrt-index:{nodeId}` / `matter-index:{containerId}` via
the agentd gateway; indexing dispatch is agentd-only and the manifests set
`AGENTD_BASE_URL=http://agentd`) and returns a minimal ack. There is no
status-read endpoint and no `index_jobs` table — indexing status is read from
the graph
(`nodes.indexed_at` / graph presence), and run history lives in the actors'
journaled state (inspector surfaces, ADR 0062). Do not reintroduce an
`index_jobs` read/write, a `GET /nodes/{id}/indexing` status route, or a
dedicated `/nodes/{id}/indexing` trigger route.

## Agent Session Plane (apps/agentd + apps/luis)

The production agent runtime is the in-cluster **agentd** (ADR 0022 → 0028/0034/0037):
a gateway plus a stateless harness pool over the FoundationDB session log. The
`Luis` harness (`apps/luis`) is a stateless process driven per turn over
loopback HTTP/NDJSON. The former Cloudflare Agents worker (`apps/agents`) was
deprecated by ADR 0028 and has been removed from the repo.

Sessions are addressed canonically at the agentd surface (ADR 0022 amendment,
#1736): `POST /v1/sessions/{sessionId}/commands`, `GET /v1/sessions/{sessionId}/events?after=N`,
and the WebSocket AG-UI stream `GET /v1/sessions/{sessionId}/stream?after=N`.
`{sessionId}` is the `node_type = "session"` node ID from `api-rs` — there is
no separate session ID.

Only `kind = "chat"` and `kind = "report"` are top-level sessions in v2
(ADR 0058, amending ADR 0033 D5); `graph` stays retired per ADR 0011.

**Sessions data-plane facade (`api-rs`):** the facade is CRUD-only and never
discriminates by harness implementation.

| Endpoint | Purpose |
|----------|---------|
| `POST /sessions` | Create-and-start boundary. v2 accepts `kind = "chat"` and `kind = "report"`; report sessions are non-conversational generation runs started via the `report.start` command (create/start decoupled, ADR 0043). `graph` is rejected / retired. Authorizes via FGA, creates the session node, and dispatches to the agentd actor surface. |
| `GET /sessions` | List chat sessions visible to the caller (FGA-scoped). |
| `GET /sessions/{id}` | Read a session node. |
| `PATCH /sessions/{id}` | Update mutable metadata (status, title, etc.). |

There is intentionally **no** `GET /sessions/{id}/access` preflight — all `apps/api-rs` endpoints are already JWT-protected, the normal `GET /sessions/{id}` read already runs the same FGA check a preflight would expose, and a dedicated preflight would invite divergence between the two paths.

Session `kind` lives as metadata on the session node only — for runtime/UI routing. MCP tooling MUST build on these generic session primitives and graph-native data-plane APIs, never on `chat.start` / job-id concepts. (The legacy `report.create` / `graph.start` action writers and the `luis_jobs` / `LuisJobSpec` job-spec surface were removed per ADR 0033 D5; do not reintroduce them. `report.start` is the sanctioned path for `kind = "report"` sessions.)

**Auth direction.**
- Browser/user requests create and read session nodes through `api-rs` `/sessions` with normal WorkOS user JWT auth; the same bearer rides on agentd connects/commands (ADR 0023).
- The runtime's data-plane authority is the per-session, per-turn scoped **`obo_agent` token** minted by `api-rs` (ADR 0023 D6 / ADR 0037): agentd's `OboAgentAuthHook` exchanges the user's bearer at `POST /sessions/{id}/agent-token`, and the harness uses only that minted token for graph/MCP/LLM-proxy work — never the user token as a runtime credential.
- `api-rs` authorizes agent-originated calls at normal endpoint/tool boundaries using agent principal, session node/container context, delegated user/FGA context where applicable, and tool constraints.

## REST API (apps/api-rs/)

**Framework:** Axum on Rust.
**Database:** Postgres (Atlas migrations at `packages/db/migrations/`; canonical schema `packages/db/schema.sql`).
**Auth:** WorkOS JWT (Bearer token).

**Module structure** (`src/modules/`) — current code with target API guardrails:

```
src/modules/
  nodes/       → CRUD for the universal `nodes` table; target home for
                 node-scoped upload/blob, indexing, and ontology routes.
  edges/       → Typed relations between nodes.
  graph/       → Authorized graph traversal and internal graph operations.
  ontology/    → Current ontology implementation plus target global/scoped ontology surface.
  indexing/    → Trigger-only indexing dispatch (ADR 0009): the `index.start`
                 action on `POST /nodes/{node_id}/actions` fires the Restate
                 invocation and returns a minimal ack. No status-read surface
                 and no `index_jobs` table — status is read from the graph.
```

The `/llm` proxy no longer lives here: it is deployed as the standalone
`apps/llm-proxy` service so api-rs stops carrying LLM provider keys. Its
external routes are unchanged — only the host moved. Browser telemetry
likewise no longer transits api-rs; it is forwarded same-origin by the
`apps/web` Cloudflare Worker (`POST /_telemetry/v1/{traces,logs}`).

Some legacy modules remain in the implementation during the migration (`admin/`, `actions/`, `search.rs`, `upload/`). Maintain existing code when necessary, but do not add new functionality, route examples, or agent guidance against legacy route families. New API work and documentation should follow the finalized target inventory in `api-routes.md`.

**Key middleware (`src/middleware/`):**
- `require_auth` — validates the JWT, sets user context.
- `require_node_access` — fetches the target node and enforces WorkOS FGA permissions on it.

Lower-level helpers live in `src/clients/` (`workos_fga.rs`, `jwks.rs`, …).
