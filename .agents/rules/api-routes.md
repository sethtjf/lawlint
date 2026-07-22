---
paths:
  - apps/api-rs/src/**/*.rs
---

# API Route Patterns

## Adding Endpoints

Routes are defined as Axum handlers in module route files under `apps/api-rs/src/modules/`:

```rust
pub async fn my_handler(
    State(state): State<AppState>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> Result<Json<MyResponse>, AppError> {
    // Handle request
    Ok(Json(response))
}
```

## Target Route Inventory

For new API work and forward-looking docs, implement and document routes against this finalized target surface. Some legacy routes may still exist during migration; maintain them only as needed and do not add new features or examples against them.

| Area | Routes |
|------|--------|
| Public probes | `GET /health`, `GET /healthz`, `GET /ready` |
| Internal | `GET /internal/metrics`, `GET /internal/ready`, `GET /openapi.json` |
| Organizations | `POST /organizations` |
| Organization ontology | `GET/POST /organizations/{organization_id}/ontology`, `GET /organizations/{organization_id}/ontology/resolutions` |
| Global ontology catalog | `GET/POST /ontologies`, `GET /ontologies/{ontology_id}`, `POST /ontologies/{ontology_id}/versions` |
| Nodes | `GET/POST /nodes`, `GET/PATCH/DELETE /nodes/{node_id}` |
| Node overview | `GET /nodes/{id}/overview` (`nodes.overview`) — hybrid read-only overview projection for organization-root / matter / session / file nodes (ADR 0033 D3, generalized from the matter-only `matters.overview`) |
| File upload/blob | `POST /files/{container_id}/uploads`, `GET /files/{file_id}/blob` |
| Node indexing | trigger only via the `index.start` action on `POST /nodes/{node_id}/actions` — no status-read route |
| Node activity | `GET /nodes/{node_id}/activity` (`activity.list`) — read-only recency projection over a matter's session/file descendants (ADR 0033); not a stored event surface |
| Node ontology | `GET/POST /nodes/{node_id}/ontology` |
| Edges | `GET/POST /edges`, `DELETE /edges/{edge_id}` |
| Graph | `POST /graph` |

The `/llm` proxy (`POST /llm/v1/chat/completions`, `GET /llm/v1/models`)
keeps the same external route but is no longer served by api-rs — it is
deployed as the standalone `apps/llm-proxy` service and documented there.
Browser telemetry is likewise no longer served by api-rs: it is forwarded
same-origin by the `apps/web` Cloudflare Worker (`POST /_telemetry/v1/{traces,logs}`).
Do not re-add their handlers, routers, or provider secrets to api-rs.

Legacy route families scheduled for removal from the target surface include `/admin`, `/search`, `/actions`, `/orgs`, `/platform`, public `/metrics`, `/upload`, `/usage`, `/nodes/{id}/children`, legacy node indexing subroutes, `/graph/heatmap`, `/usage/limits`, `/usage/events`, and `/t/v1/*`. Do not add new functionality, route examples, generated-client guidance, or public docs for these legacy families.

## Requirements

- MUST extract `AuthUser` to get authenticated user
- MUST scope queries by `auth.organization_id` for multi-tenant isolation
- SHOULD follow REST conventions for resource naming
- MUST validate request body with serde deserialization
